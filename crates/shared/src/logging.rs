use tracing_subscriber::EnvFilter;

pub fn setup_logging() {
    let target_level = "info";
    let pkg_name = env!("CARGO_PKG_NAME");

    let filter_str = format!("{},{}=debug", target_level, pkg_name).replace("-", "_");

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(filter_str.clone())),
        )
        .init();
}
