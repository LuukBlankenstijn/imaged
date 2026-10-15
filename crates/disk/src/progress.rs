use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Progress {
    pub fraction: f64,
    pub bytes_per_second: f64,
    pub remaining: Option<Duration>,
}
