#[macro_export]
macro_rules! setup_logging {
    // Base case: only the default target level
    ($target_level:expr) => {
        $crate::setup_logging!($target_level, {});
    };
    // With extra packages and levels
    ($target_level:expr, {$($extra_pkg:path => $extra_level:expr),* $(,)?}) => {{
        let pkg_name = env!("CARGO_PKG_NAME");
        let mut filter_str = format!("{},{pkg}={level}", "warn", pkg = pkg_name.replace("-", "_"), level = $target_level);
        $(
            let _ = || { use $extra_pkg as _; };
            filter_str = format!(
                "{},{}={}",
                filter_str,
                stringify!($extra_pkg).replace("::", "_"),
                $extra_level
            );
        )*
        $crate::fmt()
            .with_env_filter(
                $crate::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| $crate::EnvFilter::new(filter_str)),
            )
            .init();
    }};
}
