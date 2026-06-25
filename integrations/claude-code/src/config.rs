use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const PROJECT_FILE: &str = "lns-sandbox.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub auto_allow: bool,
    pub package_installs: bool,
    pub network_clis: bool,
    pub bypass: Vec<String>,
    pub force: Vec<String>,
    pub images: BTreeMap<String, String>,
    pub mounts: Vec<String>,
    pub env_forward: Vec<String>,
    pub cpus: Option<u32>,
    pub mem: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auto_allow: true,
            package_installs: true,
            network_clis: false,
            bypass: Vec::new(),
            force: Vec::new(),
            images: BTreeMap::new(),
            mounts: Vec::new(),
            env_forward: Vec::new(),
            cpus: None,
            mem: None,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    auto_allow: Option<bool>,
    package_installs: Option<bool>,
    network_clis: Option<bool>,
    bypass: Option<Vec<String>>,
    force: Option<Vec<String>>,
    images: Option<BTreeMap<String, String>>,
    mounts: Option<Vec<String>>,
    env_forward: Option<Vec<String>>,
    cpus: Option<u32>,
    mem: Option<String>,
}

impl Config {
    pub fn load(start_dir: &Path) -> Self {
        let mut config = Config::default();
        if let Some(raw) = global_path().and_then(|p| read_raw(&p)) {
            config = merge(config, raw);
        }
        if let Some(raw) = find_project_file(start_dir).and_then(|p| read_raw(&p)) {
            config = merge(config, raw);
        }
        config
    }

    pub fn image_override(&self, runtime_key: &str) -> Option<&str> {
        self.images.get(runtime_key).map(String::as_str)
    }
}

fn merge(mut base: Config, raw: RawConfig) -> Config {
    if let Some(v) = raw.auto_allow {
        base.auto_allow = v;
    }
    if let Some(v) = raw.package_installs {
        base.package_installs = v;
    }
    if let Some(v) = raw.network_clis {
        base.network_clis = v;
    }
    if let Some(v) = raw.bypass {
        base.bypass.extend(v);
    }
    if let Some(v) = raw.force {
        base.force.extend(v);
    }
    if let Some(v) = raw.images {
        base.images.extend(v);
    }
    if let Some(v) = raw.mounts {
        base.mounts.extend(v);
    }
    if let Some(v) = raw.env_forward {
        base.env_forward.extend(v);
    }
    if raw.cpus.is_some() {
        base.cpus = raw.cpus;
    }
    if raw.mem.is_some() {
        base.mem = raw.mem;
    }
    base
}

fn read_raw(path: &Path) -> Option<RawConfig> {
    let text = std::fs::read_to_string(path).ok()?;
    match toml::from_str(&text) {
        Ok(raw) => Some(raw),
        Err(err) => {
            eprintln!("lns-cc: ignoring invalid config {}: {err}", path.display());
            None
        }
    }
}

fn find_project_file(start_dir: &Path) -> Option<PathBuf> {
    for dir in start_dir.ancestors() {
        let candidate = dir.join(PROJECT_FILE);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn global_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("lns-sandbox").join("config.toml"))
}

pub fn normalize_mount(spec: &str) -> String {
    if !spec.contains(':') {
        format!("{spec}:{spec}:ro")
    } else if spec.ends_with(":ro") || spec.ends_with(":rw") {
        spec.to_string()
    } else {
        format!("{spec}:ro")
    }
}

pub fn resolve_forward_env(names: &[String]) -> Vec<(String, String)> {
    names
        .iter()
        .filter_map(|name| {
            if looks_secret(name) {
                eprintln!(
                    "lns-cc: not forwarding `{name}` — route secret-looking env vars through the lns credential flow, not env_forward"
                );
                return None;
            }
            std::env::var(name).ok().map(|value| (name.clone(), value))
        })
        .collect()
}

fn looks_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.ends_with("_KEY")
        || [
            "TOKEN",
            "SECRET",
            "PASSWORD",
            "PASSWD",
            "CREDENTIAL",
            "APIKEY",
        ]
        .iter()
        .any(|needle| upper.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> RawConfig {
        toml::from_str(toml).unwrap()
    }

    #[test]
    fn default_enables_tier_a_and_package_installs_only() {
        let c = Config::default();
        assert!(c.auto_allow);
        assert!(c.package_installs);
        assert!(!c.network_clis);
        assert!(c.bypass.is_empty());
        assert!(c.images.is_empty());
    }

    #[test]
    fn merge_overrides_scalars_and_extends_lists() {
        let raw = parse(
            r#"
            auto_allow = false
            network_clis = true
            bypass = ["git", "ssh"]
            mounts = ["/data:ro"]
            cpus = 4
            mem = "2g"
            [images]
            python = "python:3.13"
            "#,
        );
        let c = merge(Config::default(), raw);
        assert!(!c.auto_allow);
        assert!(c.network_clis);
        assert!(c.package_installs);
        assert_eq!(c.bypass, vec!["git".to_string(), "ssh".to_string()]);
        assert_eq!(c.mounts, vec!["/data:ro".to_string()]);
        assert_eq!(c.cpus, Some(4));
        assert_eq!(c.mem.as_deref(), Some("2g"));
        assert_eq!(c.image_override("python"), Some("python:3.13"));
        assert_eq!(c.image_override("node"), None);
    }

    #[test]
    fn later_merge_extends_lists_and_overrides_image_keys() {
        let base = merge(
            Config::default(),
            parse("bypass = [\"git\"]\n[images]\npython = \"a\"\n"),
        );
        let merged = merge(
            base,
            parse("bypass = [\"ssh\"]\n[images]\npython = \"b\"\nnode = \"c\"\n"),
        );
        assert_eq!(merged.bypass, vec!["git".to_string(), "ssh".to_string()]);
        assert_eq!(merged.image_override("python"), Some("b"));
        assert_eq!(merged.image_override("node"), Some("c"));
    }

    #[test]
    fn empty_toml_yields_defaults() {
        let c = merge(Config::default(), parse(""));
        assert_eq!(c, Config::default());
    }

    #[test]
    fn unknown_field_is_rejected() {
        assert!(toml::from_str::<RawConfig>("nonsense = true").is_err());
    }

    #[test]
    fn normalize_mount_defaults_to_read_only() {
        assert_eq!(normalize_mount("/data"), "/data:/data:ro");
        assert_eq!(normalize_mount("/host:/guest"), "/host:/guest:ro");
        assert_eq!(normalize_mount("/host:/guest:ro"), "/host:/guest:ro");
        assert_eq!(normalize_mount("/host:/guest:rw"), "/host:/guest:rw");
        assert_eq!(normalize_mount("vol:/data"), "vol:/data:ro");
    }

    #[test]
    fn looks_secret_flags_credential_shaped_names() {
        assert!(looks_secret("GITHUB_TOKEN"));
        assert!(looks_secret("aws_secret_access_key"));
        assert!(looks_secret("MY_API_KEY"));
        assert!(looks_secret("DB_PASSWORD"));
        assert!(!looks_secret("DATABASE_URL"));
        assert!(!looks_secret("NODE_ENV"));
    }

    #[test]
    fn resolve_forward_env_reads_values_and_skips_secrets() {
        let safe = format!("LNSCC_TEST_SAFE_{}", std::process::id());
        let secret = format!("LNSCC_TEST_{}_TOKEN", std::process::id());
        std::env::set_var(&safe, "value-1");
        std::env::set_var(&secret, "should-not-forward");
        let resolved =
            resolve_forward_env(&[safe.clone(), secret.clone(), "LNSCC_UNSET".to_string()]);
        assert_eq!(resolved, vec![(safe.clone(), "value-1".to_string())]);
        std::env::remove_var(&safe);
        std::env::remove_var(&secret);
    }

    #[test]
    fn load_reads_nearest_project_file_walking_up() {
        let root = std::env::temp_dir().join(format!("lnscc-cfg-{}", std::process::id()));
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join(PROJECT_FILE), "network_clis = true\n").unwrap();
        let found = find_project_file(&nested).unwrap();
        assert_eq!(found, root.join(PROJECT_FILE));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
