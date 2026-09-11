use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_CURL_IMAGE: &str = "docker.io/curlimages/curl:8.11.1";
pub const DEFAULT_ALPINE_IMAGE: &str = "docker.io/library/alpine:3.20";
pub const DEFAULT_GUEST_SUBNET: &str = "192.168.127";
pub const DEFAULT_BASE_PORT: u16 = 47200;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Images {
    #[serde(default = "default_curl")]
    pub curl: String,
    #[serde(default = "default_alpine")]
    pub alpine: String,
}

fn default_curl() -> String {
    DEFAULT_CURL_IMAGE.to_string()
}

fn default_alpine() -> String {
    DEFAULT_ALPINE_IMAGE.to_string()
}

impl Default for Images {
    fn default() -> Self {
        Self {
            curl: default_curl(),
            alpine: default_alpine(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Backend {
    pub name: String,
    pub lns: PathBuf,
    pub lns_service: PathBuf,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub expect: BTreeMap<String, String>,
    #[serde(default)]
    pub expected_differences: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub bind: Option<String>,
    pub base_port: Option<u16>,
    pub guest_subnet: Option<String>,
    #[serde(default)]
    pub images: Images,
    #[serde(default, rename = "backend")]
    pub backends: Vec<Backend>,
    /// Seconds a named case may take, overriding the budget the registry declares.
    #[serde(default)]
    pub budgets: BTreeMap<String, u64>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("{} is not a parity config", path.display()))
    }

    pub fn backend(&self, name: &str) -> Result<Backend> {
        match self.backends.iter().find(|b| b.name == name) {
            Some(backend) => Ok(backend.clone()),
            None => {
                let known: Vec<&str> = self.backends.iter().map(|b| b.name.as_str()).collect();
                bail!("no backend named {name} in the config; it holds: {known:?}")
            }
        }
    }

    pub fn guest_subnet(&self) -> String {
        self.guest_subnet
            .clone()
            .unwrap_or_else(|| DEFAULT_GUEST_SUBNET.to_string())
    }

    pub fn base_port(&self) -> u16 {
        self.base_port.unwrap_or(DEFAULT_BASE_PORT)
    }
}

pub fn parse_budget_pair(pair: &str) -> Result<(String, u64)> {
    match pair.split_once('=') {
        Some((name, seconds)) if !name.is_empty() => match seconds.parse() {
            Ok(seconds) => Ok((name.to_string(), seconds)),
            Err(_) => {
                bail!("--budget takes NAME=SECONDS, and {seconds:?} is not a number of seconds")
            }
        },
        _ => bail!("--budget takes NAME=SECONDS, not {pair:?}"),
    }
}

pub fn parse_env_pair(pair: &str) -> Result<(String, String)> {
    match pair.split_once('=') {
        Some((key, value)) if !key.is_empty() => Ok((key.to_string(), value.to_string())),
        _ => bail!("--env takes KEY=VALUE, not {pair:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("parity.toml");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    #[test]
    fn a_backend_is_a_name_two_binaries_and_an_environment() {
        let (_dir, path) = write(
            r#"
bind = "192.168.1.50"
base_port = 47300

[[backend]]
name = "netstack"
lns = "bin/lns"
lns_service = "bin/lns-service"
env = { LNS_NETDEV = "netstack" }
expect = { "loopback-witness" = "refused" }
expected_differences = ["lease-and-resolver"]
"#,
        );
        let config = Config::load(&path).unwrap();
        let backend = config.backend("netstack").unwrap();

        assert_eq!(config.bind.as_deref(), Some("192.168.1.50"));
        assert_eq!(config.base_port(), 47300);
        assert_eq!(backend.lns, PathBuf::from("bin/lns"));
        assert_eq!(backend.lns_service, PathBuf::from("bin/lns-service"));
        assert_eq!(backend.env["LNS_NETDEV"], "netstack");
        assert_eq!(backend.expect["loopback-witness"], "refused");
        assert_eq!(backend.expected_differences, vec!["lease-and-resolver"]);
    }

    #[test]
    fn the_images_and_the_subnet_have_the_defaults_this_phase_pins() {
        let (_dir, path) = write("");
        let config = Config::load(&path).unwrap();

        assert_eq!(config.images.curl, DEFAULT_CURL_IMAGE);
        assert_eq!(config.images.alpine, DEFAULT_ALPINE_IMAGE);
        assert_eq!(config.guest_subnet(), "192.168.127");
        assert_eq!(config.base_port(), DEFAULT_BASE_PORT);
    }

    #[test]
    fn a_config_may_pin_another_image_and_another_subnet() {
        let (_dir, path) = write(
            r#"
guest_subnet = "192.168.66"
images = { alpine = "docker.io/library/alpine:3.21" }
"#,
        );
        let config = Config::load(&path).unwrap();

        assert_eq!(config.images.alpine, "docker.io/library/alpine:3.21");
        assert_eq!(config.images.curl, DEFAULT_CURL_IMAGE);
        assert_eq!(config.guest_subnet(), "192.168.66");
    }

    #[test]
    fn a_backend_the_config_does_not_hold_names_the_ones_it_does() {
        let (_dir, path) = write(
            r#"
[[backend]]
name = "vmnet"
lns = "bin/lns"
lns_service = "bin/lns-service"
"#,
        );
        let config = Config::load(&path).unwrap();
        let err = config.backend("netstack").unwrap_err().to_string();

        assert!(err.contains("vmnet"), "{err}");
    }

    #[test]
    fn a_misspelled_key_is_refused_rather_than_ignored() {
        let (_dir, path) = write(
            r#"
[[backend]]
name = "netstack"
lns = "bin/lns"
lns_service = "bin/lns-service"
enviroment = { LNS_NETDEV = "netstack" }
"#,
        );
        let err = format!("{:#}", Config::load(&path).unwrap_err());
        assert!(err.contains("parity.toml"), "{err}");
    }

    #[test]
    fn a_config_may_give_a_case_more_time_than_the_registry_declares() {
        let (_dir, path) = write("budgets = { \"download-100m\" = 300 }\n");
        let config = Config::load(&path).unwrap();

        assert_eq!(config.budgets["download-100m"], 300);
    }

    #[test]
    fn a_budget_flag_is_one_case_and_a_number_of_seconds() {
        assert_eq!(
            parse_budget_pair("upload-100m=240").unwrap(),
            ("upload-100m".to_string(), 240)
        );
        assert!(parse_budget_pair("upload-100m").is_err());
        assert!(parse_budget_pair("upload-100m=soon").is_err());
        assert!(parse_budget_pair("=240").is_err());
    }

    #[test]
    fn an_environment_flag_is_one_key_and_one_value() {
        assert_eq!(
            parse_env_pair("LNS_NETDEV=netstack").unwrap(),
            ("LNS_NETDEV".to_string(), "netstack".to_string())
        );
        assert_eq!(
            parse_env_pair("LNS_GVPROXY_BIN=").unwrap(),
            ("LNS_GVPROXY_BIN".to_string(), String::new())
        );
        assert!(parse_env_pair("LNS_NETDEV").is_err());
        assert!(parse_env_pair("=netstack").is_err());
    }
}
