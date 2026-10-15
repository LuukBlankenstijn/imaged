use std::collections::{BTreeMap, HashMap};

use imaged_model::progress::HostProgress;
use tokio::sync::watch;

use crate::multicast::MulticastProgress;

#[derive(Clone, Debug, PartialEq)]
pub struct TaskProgress {
    pub task_id: i64,
    pub hosts: BTreeMap<i64, HostProgress>,
    pub multicast: Option<MulticastProgress>,
}

impl TaskProgress {
    fn new(task_id: i64) -> Self {
        Self {
            task_id,
            hosts: BTreeMap::new(),
            multicast: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    hosts: HashMap<i64, HostProgress>,
    multicast: Option<MulticastProgress>,
}

impl Snapshot {
    pub fn tasks(&self) -> Vec<TaskProgress> {
        let mut tasks = BTreeMap::new();
        for (&host_id, progress) in &self.hosts {
            tasks
                .entry(progress.task_id)
                .or_insert_with(|| TaskProgress::new(progress.task_id))
                .hosts
                .insert(host_id, progress.clone());
        }
        if let Some(multicast) = &self.multicast {
            tasks
                .entry(multicast.task_id)
                .or_insert_with(|| TaskProgress::new(multicast.task_id))
                .multicast = Some(multicast.clone());
        }
        tasks.into_values().collect()
    }
}

#[derive(Default)]
pub struct ProgressBoard {
    snapshot: watch::Sender<Snapshot>,
}

impl ProgressBoard {
    pub fn report(&self, host_id: i64, progress: HostProgress) {
        self.snapshot.send_modify(|snapshot| {
            snapshot.hosts.insert(host_id, progress);
        });
    }

    pub fn forget(&self, host_id: i64) {
        self.snapshot
            .send_if_modified(|snapshot| snapshot.hosts.remove(&host_id).is_some());
    }

    pub fn report_multicast(&self, progress: MulticastProgress) {
        self.snapshot.send_modify(|snapshot| {
            snapshot.multicast = Some(progress);
        });
    }

    pub fn end_multicast(&self) {
        self.snapshot
            .send_if_modified(|snapshot| snapshot.multicast.take().is_some());
    }

    pub fn watch(&self) -> watch::Receiver<Snapshot> {
        self.snapshot.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use imaged_model::multicast::TransferPhase;
    use imaged_model::progress::Stage;

    use super::*;

    fn host_progress(task_id: i64, fraction: f64) -> HostProgress {
        HostProgress {
            task_id,
            fraction,
            stage: Stage::Finishing,
        }
    }

    fn multicast(task_id: i64) -> MulticastProgress {
        MulticastProgress {
            task_id,
            phase: TransferPhase::Gathering,
            bytes_per_second: 0.0,
            step: 1,
            steps: 2,
            eta: Some(Duration::from_secs(9)),
            receivers: Vec::new(),
        }
    }

    #[test]
    fn hosts_are_grouped_under_the_task_they_report_for() {
        let board = ProgressBoard::default();
        board.report(1, host_progress(10, 0.5));
        board.report(2, host_progress(10, 0.25));
        board.report(3, host_progress(11, 1.0));

        let tasks = board.watch().borrow().tasks();

        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].task_id, 10);
        assert_eq!(tasks[0].hosts.keys().copied().collect::<Vec<_>>(), [1, 2]);
        assert_eq!(tasks[1].hosts[&3].fraction, 1.0);
    }

    #[test]
    fn a_host_reports_for_one_task_at_a_time() {
        let board = ProgressBoard::default();
        board.report(1, host_progress(10, 1.0));
        board.report(1, host_progress(11, 0.1));

        let tasks = board.watch().borrow().tasks();

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].task_id, 11);
    }

    #[test]
    fn a_multicast_is_shown_before_any_host_reports_and_alongside_them_after() {
        let board = ProgressBoard::default();
        board.report_multicast(multicast(10));
        let gathering = board.watch().borrow().tasks();
        board.report(1, host_progress(10, 0.5));
        let sending = board.watch().borrow().tasks();

        assert_eq!(
            gathering,
            [TaskProgress {
                multicast: Some(multicast(10)),
                ..TaskProgress::new(10)
            }]
        );
        assert_eq!(sending.len(), 1);
        assert_eq!(sending[0].multicast, Some(multicast(10)));
        assert_eq!(sending[0].hosts.len(), 1);
    }

    #[test]
    fn forgotten_hosts_and_ended_multicasts_leave_no_task_behind() {
        let board = ProgressBoard::default();
        let mut watching = board.watch();
        board.report(1, host_progress(10, 0.5));
        board.report_multicast(multicast(10));
        watching.mark_unchanged();

        board.forget(1);
        board.end_multicast();

        assert!(watching.has_changed().unwrap());
        assert!(watching.borrow_and_update().tasks().is_empty());
        board.forget(1);
        board.end_multicast();
        assert!(!watching.has_changed().unwrap());
    }
}
