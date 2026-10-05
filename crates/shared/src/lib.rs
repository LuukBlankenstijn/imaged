#[cfg(feature = "error")]
pub mod error;

#[cfg(feature = "logging")]
mod logging;
#[cfg(feature = "multicast")]
mod multicast;
#[cfg(feature = "types")]
mod types;

#[cfg(feature = "multicast")]
pub use multicast::{MULTICAST_JOIN_WINDOW, get_multicast_port, multicast_group};
#[cfg(feature = "types")]
pub use types::{ServerEvent, Task, TaskType};

#[cfg(feature = "logging")]
pub use logging::log_filter;
#[cfg(feature = "logging")]
pub use tracing_subscriber::{EnvFilter, fmt};
