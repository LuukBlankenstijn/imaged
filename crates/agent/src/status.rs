use std::collections::VecDeque;
use std::time::Duration;

use imaged_disk::{Filesystem, Progress};
use imaged_model::progress::{HostProgress, Stage};
use imaged_model::{Task, TaskType};
use scuttlecast::state::ReceiveState;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::sys::disk::PartitionTarget;

const RATE_HISTORY: usize = 120;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub mac: String,
    pub ip: Option<String>,
    pub server: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Connection {
    Connecting,
    Connected,
    Lost(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reboot {
    NotPlanned,
    Countdown { until: Instant },
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct AgentState {
    pub identity: Identity,
    pub connection: Connection,
    pub task: Option<TaskState>,
    pub reboot: Reboot,
}

impl AgentState {
    pub fn running_task(&self) -> Option<&TaskState> {
        self.task.as_ref().filter(|task| task.outcome.is_none())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Starting,
    PartitionTable,
    Partitions,
    Finishing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Waiting,
    Running,
    Done,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PartitionState {
    pub number: i64,
    pub filesystem: Filesystem,
    pub size: u64,
    pub phase: Phase,
    pub progress: Progress,
}

impl PartitionState {
    pub fn fraction(&self) -> f64 {
        match self.phase {
            Phase::Waiting => 0.0,
            Phase::Running => self.progress.fraction,
            Phase::Done => 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptPhase {
    Waiting,
    Running,
    Exited(Option<i64>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct TaskState {
    pub id: i64,
    pub kind: TaskType,
    pub image_id: Option<i64>,
    pub started: Instant,
    pub step: Step,
    pub partitions: Vec<PartitionState>,
    pub scripts: Vec<(String, ScriptPhase)>,
    pub multicast: Option<MulticastState>,
    pub outcome: Option<(Outcome, Duration)>,
}

impl TaskState {
    pub fn fraction(&self) -> f64 {
        let imaged_bytes = |p: &PartitionState| match p.filesystem {
            Filesystem::Swap => 0,
            _ => p.size,
        };
        match self.step {
            Step::Starting | Step::PartitionTable => 0.0,
            Step::Finishing => 1.0,
            Step::Partitions => {
                let total = self.partitions.iter().map(imaged_bytes).sum::<u64>().max(1);
                self.partitions
                    .iter()
                    .map(|p| imaged_bytes(p) as f64 * p.fraction())
                    .sum::<f64>()
                    / total as f64
            }
        }
    }

    pub fn current_partition(&self) -> Option<(usize, &PartitionState)> {
        self.partitions
            .iter()
            .enumerate()
            .find(|(_, p)| p.phase == Phase::Running)
    }

    fn host_progress(&self) -> HostProgress {
        let stage = match (self.step, self.current_partition()) {
            (Step::Starting | Step::PartitionTable, _) => Stage::PartitionTable,
            (Step::Partitions, Some((index, partition))) => Stage::Partition {
                number: partition.number,
                index,
                count: self.partitions.len(),
                bytes_per_second: partition.progress.bytes_per_second,
                eta_seconds: partition.progress.remaining.map(|r| r.as_secs()),
            },
            (Step::Partitions, None) | (Step::Finishing, _) => Stage::Finishing,
        };
        HostProgress {
            task_id: self.id,
            fraction: self.fraction(),
            stage,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MulticastState {
    pub slot: i64,
    pub receive: ReceiveState,
    pub rates: VecDeque<f64>,
    pub naks_per_second: f64,
    pub total_naks: u64,
    pub total_overflow_drops: u64,
    pub worst_loss: f64,
    pub worst_stall_ms: u32,
}

impl MulticastState {
    fn sample(&mut self, slot: i64, receive: ReceiveState, interval: Duration) {
        let previous = if slot == self.slot {
            std::mem::take(&mut self.receive)
        } else {
            ReceiveState::default()
        };
        let new_naks = receive.naks.saturating_sub(previous.naks);
        self.slot = slot;
        self.naks_per_second = new_naks as f64 / interval.as_secs_f64();
        self.total_naks += new_naks;
        self.total_overflow_drops += receive
            .overflow_drops
            .saturating_sub(previous.overflow_drops);
        self.worst_loss = self.worst_loss.max(receive.windowed_loss);
        self.worst_stall_ms = self.worst_stall_ms.max(receive.sink_stall_ms);
        if self.rates.len() == RATE_HISTORY {
            self.rates.pop_front();
        }
        self.rates.push_back(receive.bytes_per_second);
        self.receive = receive;
    }
}

#[derive(Clone)]
pub struct Status {
    state: watch::Sender<AgentState>,
    uplink: watch::Sender<Option<HostProgress>>,
}

impl Status {
    pub fn new(identity: Identity) -> Self {
        Self {
            state: watch::Sender::new(AgentState {
                identity,
                connection: Connection::Connecting,
                task: None,
                reboot: Reboot::NotPlanned,
            }),
            uplink: watch::Sender::new(None),
        }
    }

    pub fn watch(&self) -> watch::Receiver<AgentState> {
        self.state.subscribe()
    }

    pub fn uplink(&self) -> watch::Receiver<Option<HostProgress>> {
        self.uplink.subscribe()
    }

    pub fn snapshot(&self) -> AgentState {
        self.state.borrow().clone()
    }

    pub fn update(&self, change: impl FnOnce(&mut AgentState)) {
        self.state.send_modify(change);
        let progress = self
            .state
            .borrow()
            .running_task()
            .map(TaskState::host_progress);
        self.uplink.send_if_modified(|sent| {
            let changed = *sent != progress;
            *sent = progress;
            changed
        });
    }

    pub fn update_task(&self, change: impl FnOnce(&mut TaskState)) {
        self.update(|state| {
            if let Some(task) = state.task.as_mut().filter(|task| task.outcome.is_none()) {
                change(task);
            }
        });
    }

    pub fn task_started(&self, task: &Task) {
        let kind = task.work.task_type();
        self.update(|state| {
            state.reboot = Reboot::NotPlanned;
            state.task = Some(TaskState {
                id: task.id,
                kind,
                image_id: task.work.image_id(),
                started: Instant::now(),
                step: Step::Starting,
                partitions: Vec::new(),
                scripts: Vec::new(),
                multicast: matches!(kind, TaskType::Multicast | TaskType::MulticastPartition)
                    .then(MulticastState::default),
                outcome: None,
            });
        });
    }

    pub fn task_ended(&self, outcome: Outcome) {
        self.update_task(|task| task.outcome = Some((outcome, task.started.elapsed())));
    }

    pub fn partitions(&self, partitions: &[PartitionTarget]) -> Vec<PartitionProgress> {
        let mut task_id = 0;
        self.update_task(|task| {
            task_id = task.id;
            task.step = Step::Partitions;
            task.partitions = partitions
                .iter()
                .map(|p| PartitionState {
                    number: p.number,
                    filesystem: p.filesystem,
                    size: p.size,
                    phase: Phase::Waiting,
                    progress: Progress::default(),
                })
                .collect();
        });
        (0..partitions.len())
            .map(|index| PartitionProgress {
                status: self.clone(),
                task_id,
                index,
            })
            .collect()
    }

    pub fn multicast_sample(&self, slot: i64, receive: ReceiveState, interval: Duration) {
        self.update_task(|task| {
            if let Some(multicast) = task.multicast.as_mut() {
                multicast.sample(slot, receive, interval);
            }
        });
    }

    pub async fn reboot_countdown(&self, length: Duration) -> bool {
        let until = Instant::now() + length;
        self.update(|state| state.reboot = Reboot::Countdown { until });
        let mut reboot = self.state.subscribe();
        tokio::select! {
            _ = tokio::time::sleep_until(until) => true,
            _ = reboot.wait_for(|state| state.reboot == Reboot::Cancelled) => false,
        }
    }

    pub fn cancel_reboot(&self) -> bool {
        self.state.send_if_modified(|state| {
            let counting = matches!(state.reboot, Reboot::Countdown { .. });
            if counting {
                state.reboot = Reboot::Cancelled;
            }
            counting
        })
    }
}

#[derive(Clone)]
pub struct PartitionProgress {
    status: Status,
    task_id: i64,
    index: usize,
}

impl PartitionProgress {
    pub fn report(&self, progress: Progress) {
        self.status.update_task(|task| {
            if task.id == self.task_id
                && let Some(partition) = task.partitions.get_mut(self.index)
            {
                partition.phase = Phase::Running;
                partition.progress = progress;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imaged_model::Work;

    fn status() -> Status {
        Status::new(Identity {
            mac: "52:54:00:12:34:56".to_string(),
            ip: Some("10.0.0.7".to_string()),
            server: "http://10.0.0.1:8080".to_string(),
        })
    }

    fn target(number: i64, filesystem: Filesystem, size: u64) -> PartitionTarget {
        PartitionTarget {
            device: format!("/dev/sda{number}"),
            number,
            filesystem,
            size,
            fs_uuid: None,
            fs_label: None,
        }
    }

    fn halfway() -> Progress {
        Progress {
            fraction: 0.5,
            bytes_per_second: 2e6,
            remaining: Some(Duration::from_secs(30)),
        }
    }

    fn deploying(status: &Status) -> Vec<PartitionProgress> {
        status.task_started(&Task::new(7, Work::Deploy { image_id: 3 }));
        status.partitions(&[
            target(1, Filesystem::Vfat, 100),
            target(2, Filesystem::Swap, 10_000),
            target(3, Filesystem::Ext4, 300),
        ])
    }

    #[test]
    fn a_partition_advances_the_task_by_its_share_of_the_imaged_bytes() {
        let status = status();
        let partitions = deploying(&status);

        status.update_task(|task| {
            task.partitions[0].phase = Phase::Done;
            task.partitions[1].phase = Phase::Done;
        });
        partitions[2].report(halfway());

        assert_eq!(
            *status.uplink().borrow(),
            Some(HostProgress {
                task_id: 7,
                fraction: 0.625,
                stage: Stage::Partition {
                    number: 3,
                    index: 2,
                    count: 3,
                    bytes_per_second: 2e6,
                    eta_seconds: Some(30),
                },
            })
        );
    }

    #[test]
    fn partitions_without_imaged_bytes_do_not_divide_by_zero() {
        let status = status();
        status.task_started(&Task::new(7, Work::Deploy { image_id: 3 }));
        let partitions = status.partitions(&[target(1, Filesystem::Swap, 10)]);

        partitions[0].report(halfway());

        assert_eq!(
            status.uplink().borrow().as_ref().map(|p| p.fraction),
            Some(0.0)
        );
    }

    #[test]
    fn an_ended_task_stays_on_screen_but_stops_reporting_upstream() {
        let status = status();
        deploying(&status);
        status.update_task(|task| task.step = Step::Finishing);

        status.task_ended(Outcome::Done);

        let state = status.snapshot();
        assert!(state.running_task().is_none());
        assert_eq!(state.task.unwrap().outcome.unwrap().0, Outcome::Done);
        assert_eq!(*status.uplink().borrow(), None);
    }

    #[test]
    fn updates_for_an_ended_task_are_ignored() {
        let status = status();
        let partitions = deploying(&status);
        status.task_ended(Outcome::Cancelled);

        partitions[0].report(halfway());

        assert_eq!(
            status.snapshot().task.unwrap().partitions[0].phase,
            Phase::Waiting
        );
    }

    fn receive(naks: u64, loss: f64, stall_ms: u32) -> ReceiveState {
        ReceiveState {
            naks,
            windowed_loss: loss,
            sink_stall_ms: stall_ms,
            bytes_per_second: 1e6,
            ..ReceiveState::default()
        }
    }

    #[test]
    fn multicast_totals_carry_over_from_file_to_file() {
        let status = status();
        status.task_started(&Task::new(8, Work::Multicast { image_id: 3 }));
        let second = Duration::from_secs(1);

        status.multicast_sample(1, receive(10, 0.02, 40), second);
        status.multicast_sample(1, receive(30, 0.0, 0), second);
        status.multicast_sample(2, receive(5, 0.01, 300), second);

        let multicast = status.snapshot().task.unwrap().multicast.unwrap();
        assert_eq!(multicast.naks_per_second, 5.0);
        assert_eq!(multicast.total_naks, 35);
        assert_eq!(multicast.worst_loss, 0.02);
        assert_eq!(multicast.worst_stall_ms, 300);
        assert_eq!(multicast.rates.len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn an_unattended_countdown_runs_out() {
        assert!(status().reboot_countdown(Duration::from_secs(5)).await);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_the_countdown_stops_the_reboot() {
        let status = status();
        assert!(!status.cancel_reboot());

        let cancel = async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            assert!(status.cancel_reboot());
        };
        let (reboot, ()) = tokio::join!(status.reboot_countdown(Duration::from_secs(5)), cancel);

        assert!(!reboot);
        assert_eq!(status.snapshot().reboot, Reboot::Cancelled);
    }
}
