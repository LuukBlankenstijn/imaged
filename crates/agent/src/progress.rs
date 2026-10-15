use imaged_disk::{Filesystem, Progress};
use imaged_model::progress::{HostProgress, Stage};
use tokio::sync::watch;

use crate::sys::disk::PartitionTarget;

pub type Reports = watch::Sender<Option<HostProgress>>;

#[derive(Clone)]
pub struct TaskProgress {
    reports: Reports,
    task_id: i64,
}

impl TaskProgress {
    pub fn new(reports: Reports, task_id: i64) -> Self {
        Self { reports, task_id }
    }

    pub fn partition_table(&self) {
        self.report(0.0, Stage::PartitionTable);
    }

    pub fn partitions(&self, partitions: &[PartitionTarget]) -> Vec<PartitionProgress> {
        let total = partitions.iter().map(imaged_bytes).sum::<u64>().max(1) as f64;
        let mut before = 0.0;
        partitions
            .iter()
            .enumerate()
            .map(|(index, partition)| {
                let share = imaged_bytes(partition) as f64 / total;
                let progress = PartitionProgress {
                    task: self.clone(),
                    number: partition.number,
                    index,
                    count: partitions.len(),
                    before,
                    share,
                };
                before += share;
                progress
            })
            .collect()
    }

    pub fn finishing(&self) {
        self.report(1.0, Stage::Finishing);
    }

    fn report(&self, fraction: f64, stage: Stage) {
        self.reports.send_replace(Some(HostProgress {
            task_id: self.task_id,
            fraction,
            stage,
        }));
    }
}

fn imaged_bytes(partition: &PartitionTarget) -> u64 {
    match partition.filesystem {
        Filesystem::Swap => 0,
        _ => partition.size,
    }
}

#[derive(Clone)]
pub struct PartitionProgress {
    task: TaskProgress,
    number: i64,
    index: usize,
    count: usize,
    before: f64,
    share: f64,
}

impl PartitionProgress {
    pub fn report(&self, progress: Progress) {
        self.task.report(
            self.before + self.share * progress.fraction,
            Stage::Partition {
                number: self.number,
                index: self.index,
                count: self.count,
                bytes_per_second: progress.bytes_per_second,
                eta_seconds: progress.remaining.map(|remaining| remaining.as_secs()),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn partition(number: i64, filesystem: Filesystem, size: u64) -> PartitionTarget {
        PartitionTarget {
            device: format!("/dev/sda{number}"),
            number,
            filesystem,
            size,
        }
    }

    fn halfway() -> Progress {
        Progress {
            fraction: 0.5,
            bytes_per_second: 2e6,
            remaining: Some(Duration::from_secs(30)),
        }
    }

    #[test]
    fn a_partition_advances_the_task_by_its_share_of_the_imaged_bytes() {
        let (reports, latest) = watch::channel(None);
        let partitions = TaskProgress::new(reports, 7).partitions(&[
            partition(1, Filesystem::Vfat, 100),
            partition(2, Filesystem::Swap, 10_000),
            partition(3, Filesystem::Ext4, 300),
        ]);

        partitions[2].report(halfway());

        assert_eq!(
            *latest.borrow(),
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
        let (reports, latest) = watch::channel(None);
        let partitions =
            TaskProgress::new(reports, 7).partitions(&[partition(1, Filesystem::Swap, 10)]);

        partitions[0].report(halfway());

        assert_eq!(latest.borrow().as_ref().map(|p| p.fraction), Some(0.0));
    }
}
