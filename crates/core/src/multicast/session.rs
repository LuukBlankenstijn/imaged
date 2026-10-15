use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    time::Duration,
};

use imaged_model::{
    MULTICAST_JOIN_WINDOW,
    error::{AppError, Result},
    get_multicast_port,
    multicast::{ReceiverProgress, TransferPhase},
    multicast_group,
};
use scuttlecast::{
    error::ProtoError,
    sender::{Sender, Sending},
    state::{LimitingFactor, Phase, ReceiverState, TransferState},
};
use tokio::{sync::watch, time::Instant};
use tracing::{info, warn};

use super::{MulticastProgress, Worker};
use crate::domain::{host::Host, task::Task};

const PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(5);
const PORT_RELEASE_WAIT: Duration = Duration::from_secs(2);
const PORT_RETRY_INTERVAL: Duration = Duration::from_millis(50);

fn transfer_phase(phase: Phase) -> TransferPhase {
    match phase {
        Phase::Gathering => TransferPhase::Gathering,
        Phase::Sending => TransferPhase::Sending,
        Phase::Draining => TransferPhase::Draining,
    }
}

struct File {
    path: PathBuf,
    port: u16,
    size: u64,
}

impl Worker {
    pub(super) async fn session(&self, task: &Task, woken: &mut watch::Receiver<()>) -> Result {
        let image_id = task.image_id.ok_or_else(|| {
            AppError::FailedPrecondition("multicast task has no image".to_string())
        })?;
        let participants = self.running(task.id).await?;
        for &host in &participants {
            self.registry.send_task(host, task);
        }

        let files = self.files(image_id).await?;
        let targets = Targets::new(self.hosts.get_all(None).await?, &participants);
        let local_ip = interface_address(&self.interface)?;
        let group = multicast_group(task.id);
        let total_bytes = files.iter().map(|f| f.size).sum();
        let steps = files.len();
        let mut expected = participants.len();
        let mut bytes_before = 0;

        for (index, file) in files.into_iter().enumerate() {
            let running = self.running(task.id).await?.len();
            if running == 0 {
                return Ok(());
            }
            expected = expected.min(running);
            let step = TransferStep {
                task_id: task.id,
                step: index + 1,
                steps,
                bytes_before,
                total_bytes,
            };
            info!(
                task_id = task.id,
                step = step.step,
                steps,
                expected,
                "multicasting file"
            );
            let sending = bind_sender(local_ip, group, file.port, expected)
                .await?
                .send_file(file.path);
            match self.follow(sending, step, &targets, woken).await? {
                Some(joined) => expected = joined,
                None => return Ok(()),
            }
            bytes_before += file.size;
        }
        Ok(())
    }

    async fn follow(
        &self,
        mut sending: Sending,
        step: TransferStep,
        targets: &Targets,
        woken: &mut watch::Receiver<()>,
    ) -> Result<Option<usize>> {
        let kicker = sending.kicker();
        let mut progress = sending.progress();
        let mut log = tokio::time::interval_at(
            Instant::now() + PROGRESS_LOG_INTERVAL,
            PROGRESS_LOG_INTERVAL,
        );
        let mut joined = 0;
        let result = loop {
            tokio::select! {
                biased;
                result = &mut sending => break result,
                Ok(()) = woken.changed() => {
                    let running = match self.running(step.task_id).await {
                        Ok(running) => running,
                        Err(e) => {
                            warn!(task_id = step.task_id, err = %e, "could not check the multicast task, keeping it running");
                            continue;
                        }
                    };
                    if running.is_empty() {
                        return Ok(None);
                    }
                    for receiver in &progress.borrow().receivers {
                        if targets
                            .host_at(receiver.address.ip())
                            .is_some_and(|t| !running.contains(&t.host_id))
                        {
                            kicker.kick(receiver.receiver_id);
                        }
                    }
                }
                Ok(()) = progress.changed() => {
                    let state = progress.borrow_and_update();
                    joined = joined.max(state.receivers.len());
                    self.progress.report_multicast(step.progress(&state, targets));
                }
                _ = log.tick() => log_progress(&progress.borrow(), step.task_id, targets),
            }
        };
        let joined = joined.max(progress.borrow().receivers.len());
        match result {
            Ok(()) => Ok(Some(joined)),
            Err(ProtoError::TransferIncomplete {
                complete,
                participants,
            }) => {
                warn!(
                    task_id = step.task_id,
                    complete, participants, "not every receiver completed the file"
                );
                Ok(Some(complete))
            }
            Err(e) => Err(AppError::Internal(format!("multicast send failed: {e}"))),
        }
    }

    async fn files(&self, image_id: i64) -> Result<Vec<File>> {
        let partitions = self.images.get_partitions(image_id).await?;
        let slots = std::iter::once((self.image_service.get_partition_table_path(image_id), 0))
            .chain(partitions.into_iter().map(|p| {
                (
                    self.image_service
                        .get_partition_path(image_id, p.partition_number),
                    p.partition_number,
                )
            }));
        let mut files = Vec::new();
        for (path, slot) in slots {
            let size = tokio::fs::metadata(&path)
                .await
                .map_err(|e| AppError::Internal(format!("cannot read image file {path}: {e}")))?
                .len();
            files.push(File {
                path: path.into(),
                port: get_multicast_port(slot),
                size,
            });
        }
        Ok(files)
    }
}

async fn bind_sender(
    local_ip: Ipv4Addr,
    group: Ipv4Addr,
    port: u16,
    expected: usize,
) -> Result<Sender> {
    let deadline = Instant::now() + PORT_RELEASE_WAIT;
    loop {
        match Sender::builder().socket(local_ip, group, port) {
            Ok(builder) => {
                return Ok(builder
                    .min_receivers(expected)
                    .max_wait(MULTICAST_JOIN_WINDOW)
                    .build());
            }
            Err(ProtoError::Socket(e))
                if e.kind() == std::io::ErrorKind::AddrInUse && Instant::now() < deadline =>
            {
                tokio::time::sleep(PORT_RETRY_INTERVAL).await
            }
            Err(e) => {
                return Err(AppError::Internal(format!(
                    "failed to bind multicast sender: {e}"
                )));
            }
        }
    }
}

struct Target {
    host_id: i64,
    name: String,
}

struct Targets {
    participants: Vec<Target>,
    by_ip: HashMap<IpAddr, usize>,
}

impl Targets {
    fn new(hosts: Vec<Host>, participants: &[i64]) -> Self {
        let hosts: Vec<Host> = hosts
            .into_iter()
            .filter(|h| participants.contains(&h.id))
            .collect();
        let mut by_ip: HashMap<IpAddr, Option<usize>> = HashMap::new();
        for (index, host) in hosts.iter().enumerate() {
            if let Some(ip) = host.ip.as_deref().and_then(|ip| ip.parse().ok()) {
                by_ip
                    .entry(ip)
                    .and_modify(|shared| *shared = None)
                    .or_insert(Some(index));
            }
        }
        Self {
            participants: hosts
                .into_iter()
                .map(|h| Target {
                    name: format!("{} (#{})", h.name, h.id),
                    host_id: h.id,
                })
                .collect(),
            by_ip: by_ip
                .into_iter()
                .filter_map(|(ip, index)| Some((ip, index?)))
                .collect(),
        }
    }

    fn host_at(&self, ip: IpAddr) -> Option<&Target> {
        self.by_ip.get(&ip).map(|&index| &self.participants[index])
    }

    fn name(&self, receiver: &ReceiverState) -> String {
        let ip = receiver.address.ip();
        self.host_at(ip)
            .map_or_else(|| ip.to_string(), |target| target.name.clone())
    }

    fn receivers(&self, state: &TransferState) -> Vec<ReceiverProgress> {
        let culprit = limiting_receiver(state).map(|r| r.receiver_id);
        self.participants
            .iter()
            .map(|target| {
                let receiver = state.receivers.iter().find(|r| {
                    self.host_at(r.address.ip())
                        .is_some_and(|t| t.host_id == target.host_id)
                });
                ReceiverProgress {
                    host_id: target.host_id,
                    name: target.name.clone(),
                    joined: receiver.is_some(),
                    limiting: receiver.is_some_and(|r| Some(r.receiver_id) == culprit),
                    slices_behind: receiver.map_or(0, |r| r.slices_behind),
                    loss: receiver.map_or(0.0, |r| r.unrecovered_loss),
                    naks: receiver.map_or(0, |r| r.naks),
                    sink_stall_ms: receiver.map_or(0, |r| r.sink_stall_ms),
                }
            })
            .collect()
    }
}

struct TransferStep {
    task_id: i64,
    step: usize,
    steps: usize,
    bytes_before: u64,
    total_bytes: u64,
}

impl TransferStep {
    fn progress(&self, state: &TransferState, targets: &Targets) -> MulticastProgress {
        let sent = self.bytes_before + state.bytes_sent();
        let left = self.total_bytes.saturating_sub(sent);
        let bytes_per_second = state.bytes_per_second();
        MulticastProgress {
            task_id: self.task_id,
            phase: transfer_phase(state.phase),
            bytes_per_second,
            step: self.step,
            steps: self.steps,
            eta: Duration::try_from_secs_f64(left as f64 / bytes_per_second).ok(),
            receivers: targets.receivers(state),
        }
    }
}

fn interface_address(name: &str) -> Result<Ipv4Addr> {
    local_ip_address::list_afinet_netifas()
        .map_err(|e| AppError::Internal(format!("failed to list network interfaces: {e}")))?
        .into_iter()
        .find_map(|(iface, address)| match address {
            IpAddr::V4(v4) if iface == name => Some(v4),
            _ => None,
        })
        .ok_or_else(|| AppError::Internal(format!("interface {name} has no IPv4 address")))
}

fn limiting_receiver(state: &TransferState) -> Option<&ReceiverState> {
    let id = match state.limiting {
        LimitingFactor::WindowStalled { blocked_by, .. } => blocked_by,
        LimitingFactor::RateLimited { worst, .. } => worst,
        LimitingFactor::SinkStalled { receiver, .. } => receiver,
        _ => return None,
    };
    state.receivers.iter().find(|r| r.receiver_id == id)
}

fn log_progress(state: &TransferState, task_id: i64, targets: &Targets) {
    let culprit = limiting_receiver(state);
    info!(
        task_id,
        phase = %state.phase,
        mbit_per_second = state.bytes_per_second() * 8.0 / 1e6,
        percent_complete = ?state.fraction_complete().map(|f| f * 100.0),
        blocks_sent = state.blocks_sent,
        receivers = state.receivers.len(),
        parity_shards = state.parity_shards,
        limiting = %state.limiting,
        limiting_host = ?culprit.map(|r| targets.name(r)),
        limiting_host_loss = ?culprit.map(|r| r.unrecovered_loss),
        limiting_host_stall_ms = ?culprit.map(|r| r.sink_stall_ms),
        slowest_host = ?state.slowest().map(|r| (targets.name(r), r.slices_behind)),
        "multicast transfer progress"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receiver_state(receiver_id: u64, address: &str) -> ReceiverState {
        ReceiverState {
            receiver_id,
            address: address.parse().unwrap(),
            windowed_loss: 0.0,
            unrecovered_loss: 0.0,
            lifetime_loss: 0.0,
            next_needed_slice: 0,
            slices_behind: 0,
            naks: 0,
            sink_stall_ms: 0,
        }
    }

    #[test]
    fn the_limiting_factor_is_resolved_to_the_host_holding_the_deploy_up() {
        let state = TransferState {
            receivers: vec![
                receiver_state(11, "10.0.0.1:5000"),
                receiver_state(22, "10.0.0.2:5000"),
            ],
            limiting: LimitingFactor::SinkStalled {
                receiver: 22,
                stall_ms: 80,
            },
            ..TransferState::default()
        };

        assert_eq!(
            limiting_receiver(&state).map(|r| r.address.to_string()),
            Some("10.0.0.2:5000".to_string())
        );
    }

    #[test]
    fn a_healthy_transfer_blames_no_host() {
        let state = TransferState {
            receivers: vec![receiver_state(11, "10.0.0.1:5000")],
            limiting: LimitingFactor::Unconstrained,
            ..TransferState::default()
        };

        assert!(limiting_receiver(&state).is_none());
    }

    #[test]
    fn a_server_side_bottleneck_blames_no_host() {
        let state = TransferState {
            receivers: vec![receiver_state(11, "10.0.0.1:5000")],
            limiting: LimitingFactor::SourceStarved { read_wait_ms: 40 },
            ..TransferState::default()
        };

        assert!(limiting_receiver(&state).is_none());
    }

    fn host(id: i64, ip: &str) -> Host {
        Host::new(
            id,
            format!("lab-{id:02}"),
            format!("mac-{id}"),
            0,
            Some(ip.into()),
        )
    }

    #[test]
    fn receivers_are_named_after_the_participant_at_their_address() {
        let targets = Targets::new(vec![host(7, "10.0.0.1"), host(9, "10.0.0.9")], &[7]);

        assert_eq!(
            targets.name(&receiver_state(1, "10.0.0.1:5000")),
            "lab-07 (#7)"
        );
        assert_eq!(
            targets.name(&receiver_state(2, "10.0.0.9:5000")),
            "10.0.0.9"
        );
    }

    fn sending_state(blocks_sent: u64, blocks_per_second: f64) -> TransferState {
        TransferState {
            block_size: 100,
            blocks_sent,
            blocks_per_second,
            receivers: vec![receiver_state(11, "10.0.0.1:5000")],
            ..TransferState::default()
        }
    }

    fn step(bytes_before: u64, total_bytes: u64) -> TransferStep {
        TransferStep {
            task_id: 4,
            step: 2,
            steps: 3,
            bytes_before,
            total_bytes,
        }
    }

    fn no_targets() -> Targets {
        Targets::new(Vec::new(), &[])
    }

    #[test]
    fn the_eta_covers_the_whole_task_rather_than_the_file_being_sent() {
        let reported = step(1000, 4000).progress(&sending_state(10, 5.0), &no_targets());

        assert_eq!(reported.task_id, 4);
        assert_eq!(reported.bytes_per_second, 500.0);
        assert_eq!(reported.eta, Some(Duration::from_secs(4)));
        assert_eq!((reported.step, reported.steps), (2, 3));
    }

    #[test]
    fn a_stalled_transfer_reports_progress_without_an_eta() {
        let reported = step(1000, 4000).progress(&sending_state(0, 0.0), &no_targets());

        assert_eq!(reported.eta, None);
    }

    #[test]
    fn an_overshot_transfer_has_nothing_left_to_wait_for() {
        let reported = step(0, 500).progress(&sending_state(10, 1.0), &no_targets());

        assert_eq!(reported.eta, Some(Duration::ZERO));
    }

    #[test]
    fn each_participant_is_reported_with_its_receiver_and_the_one_holding_the_transfer_up() {
        let targets = Targets::new(
            vec![
                host(7, "10.0.0.1"),
                host(8, "10.0.0.2"),
                host(9, "10.0.0.9"),
            ],
            &[7, 8, 9],
        );
        let mut stalled = receiver_state(22, "10.0.0.2:5000");
        stalled.sink_stall_ms = 80;
        let state = TransferState {
            receivers: vec![receiver_state(11, "10.0.0.1:5000"), stalled],
            limiting: LimitingFactor::SinkStalled {
                receiver: 22,
                stall_ms: 80,
            },
            ..TransferState::default()
        };

        let receivers = targets.receivers(&state);
        let row = |id: i64| receivers.iter().find(|h| h.host_id == id).unwrap();

        assert!(row(7).joined && !row(7).limiting);
        assert!(row(8).joined && row(8).limiting);
        assert_eq!(row(8).sink_stall_ms, 80);
        assert!(!row(9).joined && !row(9).limiting);
    }

    #[test]
    fn hosts_sharing_an_address_are_never_matched_to_a_receiver() {
        let targets = Targets::new(vec![host(7, "10.0.0.1"), host(8, "10.0.0.1")], &[7, 8]);
        let state = TransferState {
            receivers: vec![receiver_state(11, "10.0.0.1:5000")],
            ..TransferState::default()
        };

        assert!(targets.receivers(&state).iter().all(|h| !h.joined));
        assert!(targets.host_at("10.0.0.1".parse().unwrap()).is_none());
    }

    #[test]
    fn the_loopback_interface_resolves_to_its_ipv4_address() {
        assert_eq!(interface_address("lo").unwrap(), Ipv4Addr::LOCALHOST);
    }

    #[test]
    fn an_unknown_interface_is_an_error_rather_than_a_silent_default() {
        assert!(interface_address("definitely-not-an-interface").is_err());
    }
}
