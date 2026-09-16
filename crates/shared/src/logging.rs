/// Builds an `EnvFilter` directive list that silences dependencies at `warn`
/// while raising our own crates to `level`. Crates absent from `crates` stay at
/// `warn`, which is why a crate that logs progress must be listed.
pub fn log_filter(crates: &[&str], level: &str) -> String {
    crates.iter().fold("warn".to_string(), |directives, name| {
        format!("{directives},{}={level}", name.replace('-', "_"))
    })
}

#[macro_export]
macro_rules! setup_logging {
    ($target_level:expr) => {
        $crate::setup_logging!($target_level, []);
    };
    ($target_level:expr, [$($extra_crate:literal),* $(,)?]) => {{
        let filter_str = $crate::log_filter(
            &[env!("CARGO_PKG_NAME") $(, $extra_crate)*],
            &$target_level.to_string(),
        );
        $crate::fmt()
            .with_env_filter(
                $crate::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| $crate::EnvFilter::new(filter_str)),
            )
            .init();
    }};
}

#[cfg(test)]
mod tests {
    use super::log_filter;

    #[test]
    fn every_named_crate_is_raised_above_the_warn_default() {
        assert_eq!(
            log_filter(&["imaged-server", "imaged_core"], "info"),
            "warn,imaged_server=info,imaged_core=info"
        );
    }

    #[test]
    fn a_lone_crate_leaves_every_dependency_at_warn() {
        assert_eq!(log_filter(&["imaged-tftp"], "debug"), "warn,imaged_tftp=debug");
    }
}
