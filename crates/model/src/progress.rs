use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostProgress {
    pub task_id: i64,
    pub fraction: f64,
    pub stage: Stage,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    PartitionTable,
    Partition {
        number: i64,
        index: usize,
        count: usize,
        bytes_per_second: f64,
        eta_seconds: Option<u64>,
    },
    Finishing,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_progress_round_trips_through_json() {
        let progress = HostProgress {
            task_id: 4,
            fraction: 0.25,
            stage: Stage::Partition {
                number: 2,
                index: 1,
                count: 3,
                bytes_per_second: 1.5e8,
                eta_seconds: Some(42),
            },
        };

        let json = serde_json::to_string(&progress).unwrap();

        assert_eq!(
            serde_json::from_str::<HostProgress>(&json).unwrap(),
            progress
        );
    }
}
