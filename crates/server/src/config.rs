use std::{
    collections::BTreeMap,
    fmt::Display,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    str::FromStr,
};

use imaged_server::{Config, Dashboard, Listeners};
use serde::{Deserialize, Deserializer, de::Error as _};
use toml_file_refs::ConfigLoader;
use tracing::level_filters::LevelFilter;

const DEFAULT_MACHINE: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080);
const DEFAULT_DASHBOARD: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8081);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    #[serde(default = "default_log_level", deserialize_with = "parsed")]
    pub log_level: LevelFilter,
    #[serde(default = "default_data_dir")]
    data_dir: PathBuf,
    #[serde(default = "default_multicast_interface")]
    multicast_interface: String,
    public_url: String,
    #[serde(default)]
    script_proxy: Option<String>,
    #[serde(default)]
    listen: Listen,
    #[serde(default)]
    script_variables: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Listen {
    machine: SocketAddr,
    #[serde(deserialize_with = "dashboard")]
    dashboard: Dashboard,
    tftp: Option<SocketAddr>,
    proxy: Option<SocketAddr>,
}

impl Default for Listen {
    fn default() -> Self {
        Self {
            machine: if dioxus::cli_config::is_cli_enabled() {
                dioxus::cli_config::fullstack_address_or_localhost()
            } else {
                DEFAULT_MACHINE
            },
            dashboard: Dashboard::Separate(DEFAULT_DASHBOARD),
            tftp: None,
            proxy: None,
        }
    }
}

fn default_log_level() -> LevelFilter {
    LevelFilter::INFO
}

fn default_data_dir() -> PathBuf {
    PathBuf::from(".")
}

fn default_multicast_interface() -> String {
    "lo".to_string()
}

fn parsed<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: Display,
{
    String::deserialize(deserializer)?
        .parse()
        .map_err(D::Error::custom)
}

fn dashboard<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Dashboard, D::Error> {
    let setting = String::deserialize(deserializer)?;
    match setting.as_str() {
        "machine" => Ok(Dashboard::OnMachineListener),
        "off" => Ok(Dashboard::Off),
        address => address.parse().map(Dashboard::Separate).map_err(|_| {
            D::Error::custom(format!(
                "{setting:?} is not \"machine\", \"off\" or an address such as \"{DEFAULT_DASHBOARD}\""
            ))
        }),
    }
}

impl FileConfig {
    pub fn load(path: &Path) -> io::Result<Self> {
        ConfigLoader::new()
            .load(path)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
    }

    pub fn into_server(self) -> io::Result<(Config, Listeners)> {
        let script_proxy = match (self.script_proxy, self.listen.proxy) {
            (Some(external), _) => Some(external),
            (None, Some(bind)) if bind.ip().is_unspecified() => {
                let public_url: axum::http::Uri = self.public_url.parse().map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("public_url {:?}: {error}", self.public_url),
                    )
                })?;
                let host = public_url.host().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("public_url {:?} has no host", self.public_url),
                    )
                })?;
                Some(format!("http://{host}:{}", bind.port()))
            }
            (None, Some(bind)) => Some(format!("http://{bind}")),
            (None, None) => None,
        };

        let mut config = Config::new(self.data_dir, self.multicast_interface);
        config.script_proxy = script_proxy;
        config.script_variables = self.script_variables;

        let mut listeners = Listeners::new(self.listen.machine, self.public_url);
        listeners.dashboard = self.listen.dashboard;
        listeners.tftp = self.listen.tftp;
        listeners.proxy = self.listen.proxy;
        Ok((config, listeners))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ConfigFile(PathBuf);

    impl ConfigFile {
        fn new(name: &str, contents: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("imaged-config-{}-{name}.toml", std::process::id()));
            std::fs::write(&path, contents).unwrap();
            Self(path)
        }
    }

    impl Drop for ConfigFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn server(name: &str, contents: &str) -> io::Result<(Config, Listeners)> {
        FileConfig::load(&ConfigFile::new(name, contents).0)?.into_server()
    }

    fn load_error(name: &str, contents: &str) -> String {
        let error = FileConfig::load(&ConfigFile::new(name, contents).0)
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        error.to_string()
    }

    #[test]
    fn the_dashboard_listens_on_loopback_apart_from_machines_by_default() {
        let (_, listeners) = server("defaults", "public_url = \"http://10.0.0.1:8080\"").unwrap();
        assert_eq!(listeners.machine, DEFAULT_MACHINE);
        assert_eq!(listeners.dashboard, Dashboard::Separate(DEFAULT_DASHBOARD));
        assert!(DEFAULT_DASHBOARD.ip().is_loopback());
    }

    #[test]
    fn the_dashboard_can_share_the_machine_listener_move_or_be_turned_off() {
        for (setting, expected) in [
            ("machine", Dashboard::OnMachineListener),
            ("off", Dashboard::Off),
            (
                "0.0.0.0:9000",
                Dashboard::Separate("0.0.0.0:9000".parse().unwrap()),
            ),
            (
                "[::1]:9000",
                Dashboard::Separate("[::1]:9000".parse().unwrap()),
            ),
        ] {
            let (_, listeners) = server(
                "dashboard",
                &format!("public_url = \"http://10.0.0.1:8080\"\nlisten.dashboard = \"{setting}\""),
            )
            .unwrap();
            assert_eq!(listeners.dashboard, expected, "{setting}");
        }
    }

    #[test]
    fn an_unknown_dashboard_setting_is_rejected() {
        let error = load_error(
            "bad-dashboard",
            "public_url = \"http://10.0.0.1:8080\"\nlisten.dashboard = \"shared\"",
        );
        assert!(error.contains("listen.dashboard"), "{error}");
        assert!(error.contains("\"shared\""), "{error}");
    }

    #[test]
    fn public_url_is_required() {
        let error = load_error("no-public-url", "multicast_interface = \"eth0\"");
        assert!(error.contains("public_url"), "{error}");
    }

    #[test]
    fn an_unknown_log_level_is_rejected() {
        let error = load_error(
            "bad-log-level",
            "public_url = \"http://10.0.0.1:8080\"\nlog_level = \"verbose\"",
        );
        assert!(error.contains("log_level"), "{error}");
    }

    #[test]
    fn log_level_accepts_every_tracing_level() {
        for level in ["off", "error", "warn", "info", "debug", "trace"] {
            let file = ConfigFile::new(
                "log-level",
                &format!("public_url = \"http://10.0.0.1:8080\"\nlog_level = \"{level}\""),
            );
            assert_eq!(
                FileConfig::load(&file.0).unwrap().log_level.to_string(),
                level
            );
        }
    }

    #[test]
    fn scripts_reach_an_unspecified_proxy_bind_through_the_public_url_host() {
        let (config, _) = server(
            "proxy",
            "public_url = \"http://10.0.0.1:8080\"\nlisten.proxy = \"0.0.0.0:3128\"",
        )
        .unwrap();
        assert_eq!(config.script_proxy.as_deref(), Some("http://10.0.0.1:3128"));
    }
}
