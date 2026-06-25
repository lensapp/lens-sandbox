use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::FromArgMatches;
use lns_ipc::{MountSpec, PortPublish};
use serde::{Deserialize, Serialize};

use crate::cli::RunArgs;
use crate::command::{CommandSpec, RunCtx, RunFuture, subcommand};

#[derive(clap::Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(clap::Subcommand)]
pub enum ConfigCommand {
    #[command(
        about = "Set a default; list keys (run.env, run.volume, run.publish) replace all previous values."
    )]
    Set(ConfigSetArgs),
    #[command(about = "Print a default's value(s); exits 1 when the key is not set.")]
    Get(ConfigKeyArgs),
    #[command(about = "Remove a default.")]
    Unset(ConfigKeyArgs),
    #[command(about = "List every configured default.")]
    List,
}

const CONFIG_KEY_HELP: &str =
    "Config key: run.cpus, run.mem, run.registry, run.env, run.volume, or run.publish.";

#[derive(clap::Args)]
pub struct ConfigSetArgs {
    #[arg(value_parser = ConfigKey::parse, help = CONFIG_KEY_HELP)]
    pub key: ConfigKey,
    #[arg(
        required = true,
        help = "Value(s) to store; each is validated like the matching `lns run` flag."
    )]
    pub values: Vec<String>,
}

#[derive(clap::Args)]
pub struct ConfigKeyArgs {
    #[arg(value_parser = ConfigKey::parse, help = CONFIG_KEY_HELP)]
    pub key: ConfigKey,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConfigKey {
    RunCpus,
    RunMem,
    RunRegistry,
    RunEnv,
    RunVolume,
    RunPublish,
}

impl ConfigKey {
    pub const ALL: [ConfigKey; 6] = [
        ConfigKey::RunCpus,
        ConfigKey::RunMem,
        ConfigKey::RunRegistry,
        ConfigKey::RunEnv,
        ConfigKey::RunVolume,
        ConfigKey::RunPublish,
    ];

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "run.cpus" => Ok(ConfigKey::RunCpus),
            "run.mem" => Ok(ConfigKey::RunMem),
            "run.registry" => Ok(ConfigKey::RunRegistry),
            "run.env" => Ok(ConfigKey::RunEnv),
            "run.volume" => Ok(ConfigKey::RunVolume),
            "run.publish" => Ok(ConfigKey::RunPublish),
            other => Err(format!(
                "unknown config key {other:?}; expected run.cpus, run.mem, run.registry, run.env, run.volume, or run.publish"
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ConfigKey::RunCpus => "run.cpus",
            ConfigKey::RunMem => "run.mem",
            ConfigKey::RunRegistry => "run.registry",
            ConfigKey::RunEnv => "run.env",
            ConfigKey::RunVolume => "run.volume",
            ConfigKey::RunPublish => "run.publish",
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    #[serde(default, skip_serializing_if = "RunSection::is_empty")]
    pub run: RunSection,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volume: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publish: Vec<String>,
}

impl RunSection {
    fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

pub fn default_config_path() -> Result<PathBuf> {
    config_path_with(
        |k| std::env::var_os(k).map(PathBuf::from),
        dirs::config_dir(),
    )
}

pub fn config_path_with(
    env: impl Fn(&str) -> Option<PathBuf>,
    config_dir: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(p) = env("LNS_CONFIG_PATH") {
        return Ok(p);
    }
    let dir = config_dir.context("could not determine the user config directory")?;
    Ok(dir.join("lns").join("config.yaml"))
}

pub fn load(path: &Path) -> Result<ConfigFile> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ConfigFile::default()),
        Err(e) => {
            return Err(e).with_context(|| format!("reading config from {}", path.display()));
        }
    };
    serde_yaml::from_str(&raw).with_context(|| format!("parsing config at {}", path.display()))
}

fn save_atomic(cfg: &ConfigFile, path: &Path) -> Result<()> {
    let body = serde_yaml::to_string(cfg).context("serializing config")?;
    if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating config directory {}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body).with_context(|| format!("writing config to {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("installing config at {}", path.display()))?;
    Ok(())
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(subcommand::<ConfigArgs>("config").about(
        "Get and set persistent defaults, applied to `lns run` when the matching flag is absent.",
    ))
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "config",
    augment,
    run: run_command,
    announces_update_check: true,
    owns_terminal: false,
};

pub fn run_command<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = ConfigArgs::from_arg_matches(matches)?;
        let path = default_config_path()?;
        let mut out = ctx.out;
        run(&args.command, &path, &mut out)
    })
}

pub fn run(cmd: &ConfigCommand, path: &Path, writer: &mut impl Write) -> Result<i32> {
    match cmd {
        ConfigCommand::Set(args) => set(args, path, writer),
        ConfigCommand::Get(args) => get(args.key, path, writer),
        ConfigCommand::Unset(args) => unset(args.key, path, writer),
        ConfigCommand::List => list(path, writer),
    }
}

fn set(args: &ConfigSetArgs, path: &Path, writer: &mut impl Write) -> Result<i32> {
    let mut cfg = load(path)?;
    store(&mut cfg, args.key, &args.values)?;
    save_atomic(&cfg, path)?;
    writeln!(
        writer,
        "Set {} to {} in {}",
        args.key.name(),
        args.values.join(", "),
        path.display()
    )?;
    Ok(0)
}

fn get(key: ConfigKey, path: &Path, writer: &mut impl Write) -> Result<i32> {
    let cfg = load(path)?;
    let values = values_of(&cfg, key);
    if values.is_empty() {
        return Ok(1);
    }
    for v in &values {
        writeln!(writer, "{v}")?;
    }
    Ok(0)
}

fn unset(key: ConfigKey, path: &Path, writer: &mut impl Write) -> Result<i32> {
    let mut cfg = load(path)?;
    if values_of(&cfg, key).is_empty() {
        bail!("{} is not set in {}", key.name(), path.display());
    }
    clear(&mut cfg, key);
    save_atomic(&cfg, path)?;
    writeln!(writer, "Unset {} in {}", key.name(), path.display())?;
    Ok(0)
}

fn list(path: &Path, writer: &mut impl Write) -> Result<i32> {
    let cfg = load(path)?;
    let lines: Vec<String> = ConfigKey::ALL
        .iter()
        .flat_map(|&key| {
            values_of(&cfg, key)
                .into_iter()
                .map(move |v| format!("{} = {v}", key.name()))
        })
        .collect();
    if lines.is_empty() {
        writeln!(writer, "No defaults set in {}", path.display())?;
        return Ok(0);
    }
    for line in &lines {
        writeln!(writer, "{line}")?;
    }
    Ok(0)
}

fn store(cfg: &mut ConfigFile, key: ConfigKey, values: &[String]) -> Result<()> {
    match key {
        ConfigKey::RunCpus => cfg.run.cpus = Some(parse_cpus(key, single(key, values)?)?),
        ConfigKey::RunMem => cfg.run.mem = Some(parse_mem(key, single(key, values)?)?),
        ConfigKey::RunRegistry => {
            let value = single(key, values)?;
            lns_policy::registry_auth::validate_registry_host(value)
                .map_err(|e| anyhow!("invalid {} value {value:?}: {e}", key.name()))?;
            cfg.run.registry = Some(value.to_string());
        }
        ConfigKey::RunEnv => {
            cfg.run.env = validated(key, values, |s| crate::cli::parse_env_kv(s).map(|_| ()))?;
        }
        ConfigKey::RunVolume => {
            cfg.run.volume = validated(key, values, |s| lns_ipc::MountSpec::parse(s).map(|_| ()))?;
        }
        ConfigKey::RunPublish => {
            cfg.run.publish = validated(key, values, |s| {
                crate::cli::parse_publish_arg(s).map(|_| ())
            })?;
        }
    }
    Ok(())
}

fn single(key: ConfigKey, values: &[String]) -> Result<&str> {
    match values {
        [v] => Ok(v.as_str()),
        _ => bail!("{} takes a single value", key.name()),
    }
}

fn parse_number<N: std::str::FromStr<Err = std::num::ParseIntError>>(
    key: ConfigKey,
    value: &str,
) -> Result<N> {
    value
        .parse()
        .map_err(|e| anyhow!("invalid {} value {value:?}: {e}", key.name()))
}

fn parse_cpus(key: ConfigKey, value: &str) -> Result<u8> {
    match parse_number(key, value)? {
        0 => bail!("invalid {} value {value:?}: must be at least 1", key.name()),
        n => Ok(n),
    }
}

fn parse_mem(key: ConfigKey, value: &str) -> Result<usize> {
    crate::cli::parse_mem_arg(value)
        .map_err(|e| anyhow!("invalid {} value {value:?}: {e}", key.name()))
}

fn validated(
    key: ConfigKey,
    values: &[String],
    check: impl Fn(&str) -> Result<(), String>,
) -> Result<Vec<String>> {
    for v in values {
        check(v).map_err(|e| anyhow!("invalid {} value {v:?}: {e}", key.name()))?;
    }
    Ok(values.to_vec())
}

fn values_of(cfg: &ConfigFile, key: ConfigKey) -> Vec<String> {
    match key {
        ConfigKey::RunCpus => cfg.run.cpus.map(|v| v.to_string()).into_iter().collect(),
        ConfigKey::RunMem => cfg.run.mem.map(|v| v.to_string()).into_iter().collect(),
        ConfigKey::RunRegistry => cfg.run.registry.clone().into_iter().collect(),
        ConfigKey::RunEnv => cfg.run.env.clone(),
        ConfigKey::RunVolume => cfg.run.volume.clone(),
        ConfigKey::RunPublish => cfg.run.publish.clone(),
    }
}

fn clear(cfg: &mut ConfigFile, key: ConfigKey) {
    match key {
        ConfigKey::RunCpus => cfg.run.cpus = None,
        ConfigKey::RunMem => cfg.run.mem = None,
        ConfigKey::RunRegistry => cfg.run.registry = None,
        ConfigKey::RunEnv => cfg.run.env.clear(),
        ConfigKey::RunVolume => cfg.run.volume.clear(),
        ConfigKey::RunPublish => cfg.run.publish.clear(),
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct RunDefaults {
    pub cpus: Option<u8>,
    pub mem: Option<usize>,
    pub registry: Option<String>,
    pub env: Vec<String>,
    pub mounts: Vec<MountSpec>,
    pub publish: Vec<PortPublish>,
}

pub fn load_run_defaults(path: &Path) -> Result<RunDefaults> {
    let cfg = load(path)?;
    Ok(RunDefaults {
        cpus: nonzero_default(ConfigKey::RunCpus, cfg.run.cpus, path)?,
        mem: nonzero_default(ConfigKey::RunMem, cfg.run.mem, path)?,
        registry: cfg.run.registry,
        env: parsed_defaults(
            ConfigKey::RunEnv,
            &cfg.run.env,
            path,
            crate::cli::parse_env_kv,
        )?,
        mounts: parsed_defaults(
            ConfigKey::RunVolume,
            &cfg.run.volume,
            path,
            MountSpec::parse,
        )?,
        publish: parsed_defaults(
            ConfigKey::RunPublish,
            &cfg.run.publish,
            path,
            crate::cli::parse_publish_arg,
        )?,
    })
}

fn nonzero_default<N>(key: ConfigKey, value: Option<N>, path: &Path) -> Result<Option<N>>
where
    N: Copy + PartialEq + From<u8> + std::fmt::Display,
{
    if value == Some(N::from(0)) {
        bail!(
            "invalid {} default 0 in {}: must be at least 1",
            key.name(),
            path.display()
        );
    }
    Ok(value)
}

fn parsed_defaults<T>(
    key: ConfigKey,
    raw: &[String],
    path: &Path,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<Vec<T>> {
    raw.iter()
        .map(|v| {
            parse(v).map_err(|e| {
                anyhow!(
                    "invalid {} default {v:?} in {}: {e}",
                    key.name(),
                    path.display()
                )
            })
        })
        .collect()
}

pub fn apply_run_defaults(mut args: RunArgs, defaults: RunDefaults) -> RunArgs {
    args.cpus = args.cpus.or(defaults.cpus);
    args.mem = args.mem.or(defaults.mem);
    args.registry = args.registry.or(defaults.registry);
    if let Some(image) = args.image.take() {
        args.image = Some(resolve_default_registry(&image, args.registry.as_deref()));
    }
    args.env = merged_env(defaults.env, args.env);
    args.mounts = merged_mounts(defaults.mounts, args.mounts);
    args.publish = merged_publish(defaults.publish, args.publish);
    args
}

/// Qualifies a bare image reference with the configured default registry; a reference that already names a registry — or a docker.io default, which the parser already assumes — is left untouched so implicit `library/` namespacing is preserved.
pub fn resolve_default_registry(image: &str, default_registry: Option<&str>) -> String {
    let Some(registry) = default_registry else {
        return image.to_string();
    };
    if lns_policy::registry_auth::canonical_registry(registry) == "docker.io"
        || image_has_registry(image)
    {
        return image.to_string();
    }
    format!("{registry}/{image}")
}

fn image_has_registry(image: &str) -> bool {
    match image.split_once('/') {
        Some((first, _)) => first.contains('.') || first.contains(':') || first == "localhost",
        None => false,
    }
}

fn merged_env(defaults: Vec<String>, flags: Vec<String>) -> Vec<String> {
    let mut merged: Vec<String> = defaults
        .into_iter()
        .filter(|d| !flags.iter().any(|f| env_key(f) == env_key(d)))
        .collect();
    merged.extend(flags);
    merged
}

fn env_key(entry: &str) -> &str {
    entry.split_once('=').map_or(entry, |(key, _)| key)
}

fn merged_mounts(defaults: Vec<MountSpec>, flags: Vec<MountSpec>) -> Vec<MountSpec> {
    let mut merged: Vec<MountSpec> = defaults
        .into_iter()
        .filter(|d| !flags.iter().any(|f| f.target() == d.target()))
        .collect();
    merged.extend(flags);
    merged
}

fn merged_publish(defaults: Vec<PortPublish>, flags: Vec<PortPublish>) -> Vec<PortPublish> {
    let mut merged: Vec<PortPublish> = defaults
        .into_iter()
        .filter(|d| {
            !flags.iter().any(|f| {
                f.host_ip == d.host_ip && f.host_port == d.host_port && f.protocol == d.protocol
            })
        })
        .collect();
    merged.extend(flags);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn set_cmd(key: ConfigKey, values: &[&str]) -> ConfigCommand {
        ConfigCommand::Set(ConfigSetArgs {
            key,
            values: values.iter().map(|s| s.to_string()).collect(),
        })
    }

    fn run_ok(cmd: &ConfigCommand, path: &Path) -> (i32, String) {
        let mut buf = Vec::new();
        let code = run(cmd, path, &mut buf).expect("command succeeds");
        (code, String::from_utf8(buf).unwrap())
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn run_command_resolves_the_default_path_and_writes_a_stored_default() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("config.yaml");
        let _scope = crate::test_env::EnvScope::set("LNS_CONFIG_PATH", &cfg);

        let set = crate::command::build_cli()
            .try_get_matches_from(["lns", "config", "set", "run.cpus", "4"])
            .unwrap();
        let (_, set_sub) = set.subcommand().unwrap();
        let mut input: &[u8] = b"";
        let mut out: Vec<u8> = Vec::new();
        let ctx = RunCtx {
            debug: false,
            cwd: None,
            input: &mut input,
            out: &mut out,
        };
        assert_eq!(run_command(set_sub, ctx).await.unwrap(), 0);

        assert_eq!(load(&cfg).unwrap().run.cpus, Some(4));
    }

    #[test]
    fn config_key_parse_accepts_every_documented_key_and_round_trips_its_name() {
        for key in ConfigKey::ALL {
            assert_eq!(ConfigKey::parse(key.name()).unwrap(), key);
        }
    }

    #[test]
    fn config_key_parse_rejects_an_unknown_key_listing_the_valid_ones() {
        let err = ConfigKey::parse("run.bogus").unwrap_err();
        assert!(err.contains("unknown config key"), "got: {err}");
        assert!(err.contains("run.cpus"), "must list valid keys: {err}");
    }

    #[test]
    fn load_returns_an_empty_config_when_the_file_does_not_exist() {
        let dir = TempDir::new().unwrap();
        let cfg = load(&dir.path().join("config.yaml")).unwrap();
        assert_eq!(cfg, ConfigFile::default());
    }

    #[test]
    fn load_names_the_file_when_the_yaml_is_malformed() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "run: [not, a, map]\n").unwrap();
        let err = load(&path).unwrap_err();
        assert!(format!("{err:#}").contains("config.yaml"), "got: {err:#}");
    }

    #[test]
    fn load_rejects_an_unknown_field_so_a_hand_edit_typo_cannot_be_silently_ignored() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "run:\n  cpu: 2\n").unwrap();
        let err = format!("{:#}", load(&path).unwrap_err());
        assert!(err.contains("cpu"), "got: {err}");
    }

    #[test]
    fn load_surfaces_a_read_error_that_is_not_file_absence() {
        let dir = TempDir::new().unwrap();
        let err = load(dir.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("reading config from"),
            "got: {err:#}"
        );
    }

    #[test]
    fn set_creates_missing_parent_directories_and_leaves_no_tmp_sibling() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("lns").join("config.yaml");
        let (code, out) = run_ok(&set_cmd(ConfigKey::RunCpus, &["4"]), &path);
        assert_eq!(code, 0);
        assert!(out.contains("Set run.cpus to 4"), "got: {out}");
        assert_eq!(load(&path).unwrap().run.cpus, Some(4));
        assert!(!path.with_extension("tmp").exists(), "tmp file left behind");
    }

    #[test]
    fn save_fails_when_the_config_parent_is_a_file() {
        let dir = TempDir::new().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, "").unwrap();
        let err = save_atomic(&ConfigFile::default(), &blocker.join("config.yaml")).unwrap_err();
        assert!(
            format!("{err:#}").contains("creating config directory"),
            "got: {err:#}"
        );
    }

    #[test]
    fn set_fails_when_the_tmp_path_is_occupied_by_a_directory() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("config.tmp")).unwrap();
        let mut buf = Vec::new();
        let err = run(
            &set_cmd(ConfigKey::RunCpus, &["4"]),
            &dir.path().join("config.yaml"),
            &mut buf,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("writing config to"),
            "got: {err:#}"
        );
    }

    #[test]
    fn save_fails_when_the_config_path_is_a_directory() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("config.yaml")).unwrap();
        let err = save_atomic(&ConfigFile::default(), &dir.path().join("config.yaml")).unwrap_err();
        assert!(
            format!("{err:#}").contains("installing config at"),
            "got: {err:#}"
        );
    }

    #[test]
    fn set_rejects_an_empty_value_list_for_a_single_value_key() {
        let mut cfg = ConfigFile::default();
        let err = store(&mut cfg, ConfigKey::RunMem, &[]).unwrap_err();
        assert!(format!("{err:#}").contains("a single value"));
    }

    #[test]
    fn set_rejects_an_out_of_range_mem_value() {
        let mut cfg = ConfigFile::default();
        let err = store(&mut cfg, ConfigKey::RunMem, &["lots".to_string()]).unwrap_err();
        assert!(format!("{err:#}").contains("run.mem"), "got: {err:#}");
    }

    #[test]
    fn every_key_survives_a_set_get_list_unset_lifecycle() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        let seeds: [(ConfigKey, &[&str]); 6] = [
            (ConfigKey::RunCpus, &["4"]),
            (ConfigKey::RunMem, &["2048"]),
            (ConfigKey::RunRegistry, &["ghcr.io"]),
            (ConfigKey::RunEnv, &["TZ=UTC", "CI=1"]),
            (ConfigKey::RunVolume, &["cache:/var/cache:ro"]),
            (ConfigKey::RunPublish, &["8080:80"]),
        ];
        for (key, values) in seeds {
            let (code, _) = run_ok(&set_cmd(key, values), &path);
            assert_eq!(code, 0, "set {} failed", key.name());
        }
        let (_, listing) = run_ok(&ConfigCommand::List, &path);
        for needle in [
            "run.cpus = 4",
            "run.mem = 2048",
            "run.registry = ghcr.io",
            "run.env = TZ=UTC",
            "run.env = CI=1",
            "run.volume = cache:/var/cache:ro",
            "run.publish = 8080:80",
        ] {
            assert!(
                listing.lines().any(|l| l == needle),
                "missing {needle}: {listing}"
            );
        }
        for (key, values) in seeds {
            let (code, out) = run_ok(&ConfigCommand::Get(ConfigKeyArgs { key }), &path);
            assert_eq!(code, 0);
            assert_eq!(out.lines().count(), values.len(), "get {}", key.name());
            let (code, out) = run_ok(&ConfigCommand::Unset(ConfigKeyArgs { key }), &path);
            assert_eq!(code, 0, "unset {} failed", key.name());
            assert!(out.contains("Unset"), "got: {out}");
        }
        let (_, listing) = run_ok(&ConfigCommand::List, &path);
        assert!(listing.starts_with("No defaults set in "), "got: {listing}");
    }

    #[test]
    fn config_path_with_prefers_the_env_override() {
        let path = config_path_with(
            |k| (k == "LNS_CONFIG_PATH").then(|| PathBuf::from("/tmp/elsewhere.yaml")),
            Some(PathBuf::from("/home/dev/.config")),
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/tmp/elsewhere.yaml"));
    }

    #[test]
    fn config_path_with_defaults_to_lns_config_yaml_under_the_config_dir() {
        let path = config_path_with(|_| None, Some(PathBuf::from("/home/dev/.config"))).unwrap();
        assert_eq!(path, PathBuf::from("/home/dev/.config/lns/config.yaml"));
    }

    #[test]
    fn config_path_with_errors_when_no_config_dir_exists() {
        let err = config_path_with(|_| None, None).unwrap_err();
        assert!(
            format!("{err:#}").contains("config directory"),
            "got: {err:#}"
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_config_path_honours_the_lns_config_path_override() {
        let _guard = crate::test_env::EnvScope::set("LNS_CONFIG_PATH", "/tmp/override.yaml");
        assert_eq!(
            default_config_path().unwrap(),
            PathBuf::from("/tmp/override.yaml")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_config_path_lands_under_the_user_config_dir() {
        let _guard = crate::test_env::EnvScope::unset("LNS_CONFIG_PATH");
        let path = default_config_path().unwrap();
        assert!(path.ends_with("lns/config.yaml"), "got: {path:?}");
        assert!(path.is_absolute(), "got: {path:?}");
    }

    fn bare_run_args() -> RunArgs {
        RunArgs {
            image: Some("alpine".to_string()),
            name: None,
            registry: None,
            cpus: None,
            mem: None,
            policy: None,
            sandbox_user: None,
            sandbox_uid: None,
            interactive: true,
            tty: true,
            detach: false,
            detach_keys: crate::cli::DetachChord(vec![0x10, 0x11]),
            workdir: None,
            env: Vec::new(),
            env_file: Vec::new(),
            publish: Vec::new(),
            mounts: Vec::new(),
            cmd: Vec::new(),
            mount: Vec::new(),
            artifact_mounts: Vec::new(),
        }
    }

    #[test]
    fn load_run_defaults_parses_volume_and_publish_entries_into_typed_mounts() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            "run:\n  volume:\n    - cache:/var/cache:ro\n  publish:\n    - 8080:80\n",
        )
        .unwrap();
        let defaults = load_run_defaults(&path).unwrap();
        assert_eq!(
            defaults.mounts,
            vec![MountSpec::parse("cache:/var/cache:ro").unwrap()]
        );
        assert_eq!(defaults.publish[0].host_port, 8080);
        assert_eq!(defaults.publish[0].container_port, 80);
    }

    #[test]
    fn load_run_defaults_parses_a_host_bind_entry() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "run:\n  volume:\n    - /srv/data:/data:ro\n").unwrap();
        let defaults = load_run_defaults(&path).unwrap();
        assert_eq!(
            defaults.mounts,
            vec![MountSpec::parse("/srv/data:/data:ro").unwrap()]
        );
    }

    #[test]
    fn load_run_defaults_names_the_key_and_file_for_a_malformed_volume_entry() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "run:\n  volume:\n    - data:relative\n").unwrap();
        let err = format!("{:#}", load_run_defaults(&path).unwrap_err());
        assert!(err.contains("run.volume"), "got: {err}");
        assert!(err.contains("config.yaml"), "got: {err}");
    }

    #[test]
    fn load_run_defaults_names_the_key_and_file_for_a_malformed_publish_entry() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "run:\n  publish:\n    - nonsense\n").unwrap();
        let err = format!("{:#}", load_run_defaults(&path).unwrap_err());
        assert!(err.contains("run.publish"), "got: {err}");
    }

    #[test]
    fn store_rejects_a_zero_cpu_default() {
        let mut cfg = ConfigFile::default();
        let err = store(&mut cfg, ConfigKey::RunCpus, &["0".to_string()]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("run.cpus"), "got: {msg}");
        assert!(msg.contains("at least 1"), "got: {msg}");
    }

    #[test]
    fn store_rejects_a_zero_mem_default() {
        let mut cfg = ConfigFile::default();
        let err = store(&mut cfg, ConfigKey::RunMem, &["0".to_string()]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("run.mem"), "got: {msg}");
        assert!(msg.contains("at least 1 MiB"), "got: {msg}");
    }

    #[test]
    fn store_accepts_a_mem_unit_suffix_and_normalizes_to_mib() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        let (code, out) = run_ok(&set_cmd(ConfigKey::RunMem, &["2g"]), &path);
        assert_eq!(code, 0);
        assert!(out.contains("Set run.mem to 2g"), "got: {out}");
        assert_eq!(load(&path).unwrap().run.mem, Some(2048));
        let (_, shown) = run_ok(
            &ConfigCommand::Get(ConfigKeyArgs {
                key: ConfigKey::RunMem,
            }),
            &path,
        );
        assert_eq!(shown, "2048\n");
    }

    #[test]
    fn load_run_defaults_rejects_a_hand_edited_zero_cpu_naming_the_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "run:\n  cpus: 0\n").unwrap();
        let err = format!("{:#}", load_run_defaults(&path).unwrap_err());
        assert!(err.contains("run.cpus"), "got: {err}");
        assert!(err.contains("config.yaml"), "got: {err}");
        assert!(err.contains("at least 1"), "got: {err}");
    }

    #[test]
    fn load_run_defaults_rejects_a_hand_edited_zero_mem_naming_the_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "run:\n  mem: 0\n").unwrap();
        let err = format!("{:#}", load_run_defaults(&path).unwrap_err());
        assert!(err.contains("run.mem"), "got: {err}");
        assert!(err.contains("config.yaml"), "got: {err}");
    }

    #[test]
    fn apply_run_defaults_keeps_explicit_resources_over_configured_ones() {
        let mut args = bare_run_args();
        args.cpus = Some(2);
        args.mem = Some(1024);
        let resolved = apply_run_defaults(
            args,
            RunDefaults {
                cpus: Some(8),
                mem: Some(4096),
                ..RunDefaults::default()
            },
        );
        assert_eq!(resolved.cpus, Some(2));
        assert_eq!(resolved.mem, Some(1024));
    }

    #[test]
    fn store_rejects_an_invalid_registry_host() {
        let mut cfg = ConfigFile::default();
        let err = store(
            &mut cfg,
            ConfigKey::RunRegistry,
            &["https://ghcr.io".to_string()],
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("run.registry"), "got: {msg}");
        assert!(msg.contains("URL"), "got: {msg}");
    }

    #[test]
    fn resolve_default_registry_leaves_an_image_untouched_without_a_default() {
        assert_eq!(resolve_default_registry("alpine", None), "alpine");
    }

    #[test]
    fn resolve_default_registry_does_not_prepend_a_docker_io_default() {
        // docker.io is the parser's own default; prepending it would break the implicit `library/` namespacing.
        for d in ["docker.io", "index.docker.io"] {
            assert_eq!(resolve_default_registry("alpine", Some(d)), "alpine");
        }
    }

    #[test]
    fn resolve_default_registry_qualifies_a_bare_reference_with_a_non_docker_default() {
        assert_eq!(
            resolve_default_registry("alpine", Some("ghcr.io")),
            "ghcr.io/alpine"
        );
        assert_eq!(
            resolve_default_registry("alpine:3.20", Some("ghcr.io")),
            "ghcr.io/alpine:3.20"
        );
        assert_eq!(
            resolve_default_registry("org/app", Some("ghcr.io")),
            "ghcr.io/org/app"
        );
    }

    #[test]
    fn resolve_default_registry_leaves_a_fully_qualified_reference_alone() {
        for image in [
            "myreg.io/app",
            "localhost:5000/app",
            "localhost/app",
            "registry.example.test/team/app:1",
        ] {
            assert_eq!(resolve_default_registry(image, Some("ghcr.io")), image);
        }
    }

    #[test]
    fn apply_run_defaults_qualifies_a_bare_image_with_the_configured_registry() {
        let resolved = apply_run_defaults(
            bare_run_args(),
            RunDefaults {
                registry: Some("ghcr.io".into()),
                ..RunDefaults::default()
            },
        );
        assert_eq!(resolved.image.as_deref(), Some("ghcr.io/alpine"));
    }

    #[test]
    fn apply_run_defaults_prefers_the_registry_flag_over_the_configured_default() {
        let mut args = bare_run_args();
        args.registry = Some("quay.io".into());
        let resolved = apply_run_defaults(
            args,
            RunDefaults {
                registry: Some("ghcr.io".into()),
                ..RunDefaults::default()
            },
        );
        assert_eq!(resolved.image.as_deref(), Some("quay.io/alpine"));
    }

    #[test]
    fn apply_run_defaults_leaves_a_qualified_image_untouched() {
        let mut args = bare_run_args();
        args.image = Some("docker.io/library/alpine".into());
        let resolved = apply_run_defaults(
            args,
            RunDefaults {
                registry: Some("ghcr.io".into()),
                ..RunDefaults::default()
            },
        );
        assert_eq!(resolved.image.as_deref(), Some("docker.io/library/alpine"));
    }

    #[test]
    fn apply_run_defaults_keeps_an_imageless_run_imageless() {
        let mut args = bare_run_args();
        args.image = None;
        let resolved = apply_run_defaults(
            args,
            RunDefaults {
                registry: Some("ghcr.io".into()),
                ..RunDefaults::default()
            },
        );
        assert_eq!(resolved.image, None);
    }

    #[test]
    fn merged_env_keeps_configured_entries_first_and_flags_last() {
        let merged = merged_env(vec!["TZ=UTC".into(), "CI=1".into()], vec!["DEBUG=1".into()]);
        assert_eq!(merged, vec!["TZ=UTC", "CI=1", "DEBUG=1"]);
    }

    #[test]
    fn merged_mounts_keeps_the_same_volume_mounted_at_two_different_targets() {
        let mount = |spec: &str| MountSpec::parse(spec).unwrap();
        let merged = merged_mounts(vec![mount("cache:/a")], vec![mount("cache:/b")]);
        assert_eq!(
            merged.len(),
            2,
            "same name at distinct targets is not a conflict"
        );
    }

    #[test]
    fn merged_mounts_lets_a_flag_override_the_configured_mount_at_the_same_target() {
        let mount = |spec: &str| MountSpec::parse(spec).unwrap();
        let merged = merged_mounts(vec![mount("cache:/data")], vec![mount("/srv/data:/data")]);
        assert_eq!(merged, vec![mount("/srv/data:/data")]);
    }

    #[test]
    fn merged_publish_keeps_the_same_host_port_on_two_different_binds() {
        let publish = |spec: &str| crate::cli::parse_publish_arg(spec).unwrap();
        let merged = merged_publish(
            vec![publish("127.0.0.1:8080:80")],
            vec![publish("0.0.0.0:8080:90")],
        );
        assert_eq!(merged.len(), 2, "distinct host ips are distinct binds");
    }
}
