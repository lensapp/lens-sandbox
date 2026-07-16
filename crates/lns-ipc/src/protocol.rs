use std::net::IpAddr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    Ping,
    Status,
    Shutdown,
    Unknown {
        method: String,
    },
    RunImage(Box<RunImageArgs>),
    CancelRun {
        run_id: String,
    },
    RunStdin {
        run_id: String,
        bytes: Vec<u8>,
    },
    RunResize {
        run_id: String,
        rows: u16,
        cols: u16,
    },
    RunSignal {
        run_id: String,
        signal: SignalKind,
    },
    RunDetach {
        run_id: String,
    },
    ExecImage(ExecImageArgs),
    Kill {
        run: String,
        signal: SignalKind,
    },
    ListRuns,
    StopRun {
        run: String,
        timeout_secs: u64,
    },
    InspectRun {
        run: String,
    },
    RunLogs {
        run: String,
        follow: bool,
    },
    AttachRun {
        run: String,
    },
    RunStats {
        run: String,
    },
    RemoveRun {
        run: String,
    },
    RenameRun {
        run: String,
        new_name: String,
    },
    PruneRuns,
    BeginIntegrationSignIn {
        id: String,
    },
    BindIntegrationCredential {
        id: String,
    },
    ListVolumes,
    CreateVolume {
        name: String,
    },
    InspectVolume {
        name: String,
    },
    RemoveVolume {
        name: String,
    },
    PruneVolumes,
    PullImage {
        image: String,
    },
    ListImages,
    RemoveImage {
        image: String,
    },
    PruneImages,
    InspectImage {
        image: String,
    },
    TagImage {
        from: String,
        to: String,
    },
    RegistryLogin {
        registry: String,
        username: String,
        secret: String,
    },
}

/// The value decision a credential bind resolved to: a stored value, the host-detected value, or an explicit deny — all three persist per machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialBindDecision {
    Stored,
    HostDetect,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignalKind {
    Int,
    Term,
    Quit,
    Hup,
    Winch,
    Kill,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Response {
    Pong,
    Status(StatusInfo),
    ShuttingDown,
    Error {
        message: String,
    },
    RunStarted {
        run_id: String,
    },
    RunLog {
        level: LogLevel,
        verb: Option<String>,
        message: String,
    },
    RunExit {
        code: i32,
    },
    CancelAccepted,
    DetachAccepted,
    Acknowledged,
    RunList {
        runs: Vec<RunSummary>,
    },
    RunStopped {
        forced: bool,
    },
    RunInspect {
        details: Box<RunDetails>,
    },
    RunStats {
        stats: RunStatsInfo,
    },
    RunsPruned {
        removed: Vec<String>,
    },
    OauthVerification {
        verification_uri: String,
        user_code: String,
        expires_in_secs: u64,
    },
    OauthBrowserOpened {
        authorization_url: String,
    },
    OauthSignInComplete,
    OauthSignInFailed {
        reason: String,
    },
    CredentialBindComplete {
        decision: CredentialBindDecision,
    },
    CredentialBindFailed {
        reason: String,
    },
    RegistryLoginVerified,
    VolumeList {
        volumes: Vec<VolumeInfo>,
    },
    VolumeCreated {
        volume: VolumeInfo,
    },
    VolumeInspect {
        volume: VolumeInfo,
    },
    VolumeRemoved {
        name: String,
    },
    VolumesPruned {
        removed: Vec<String>,
        reclaimed_bytes: u64,
        failed: Vec<VolumePruneFailure>,
    },
    ImagePulled {
        image: ImageInfo,
    },
    ImageList {
        images: Vec<ImageInfo>,
    },
    ImageRemoved {
        reference: String,
        reclaimed_bytes: u64,
    },
    ImagesPruned {
        removed: Vec<String>,
        reclaimed_bytes: u64,
    },
    ImageInspected {
        inspection: ArtifactInspection,
    },
    ImageTagged {
        from: String,
        to: String,
    },
    RunProgress {
        verb: String,
        message: String,
        current: u64,
        /// 0 means the size of the work is unknown (render as indeterminate).
        total: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub name: String,
    pub size_bytes: u64,
    pub disk_bytes: u64,
    pub created: String,
    pub in_use_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumePruneFailure {
    pub name: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageInfo {
    pub reference: String,
    pub digest: String,
    pub size_bytes: u64,
    pub layers: u32,
    pub pulled: String,
    pub in_use_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ArtifactInspection {
    Image(ImageView),
    Bundle(BundleView),
    Sandbox(SandboxView),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageView {
    pub reference: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxView {
    pub reference: String,
    pub image: String,
    #[serde(default)]
    pub integrations: Vec<String>,
    #[serde(default)]
    pub policy_flags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleView {
    pub reference: String,
    pub sandbox_base_image: Option<String>,
    pub filesets: Vec<FilesetView>,
    pub integrations: Vec<String>,
    pub signature: SignatureView,
    pub policy_flags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesetView {
    pub name: String,
    pub mount_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureView {
    Unsigned,
    SignedTrusted,
    SignedUntrusted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: String,
    pub name: String,
    pub image: String,
    pub command: String,
    pub status: RunStatus,
    pub started: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum RunStatus {
    Running,
    Exited { code: i32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStatsInfo {
    pub cpu_permille: u32,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunDetails {
    pub summary: RunSummary,
    pub config: RunConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunConfig {
    pub cpus: u8,
    pub mem_mib: usize,
    pub policy_path: Option<String>,
    pub sandbox_user: Option<String>,
    pub sandbox_uid: Option<u32>,
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
    pub env: Vec<String>,
    pub published_ports: Vec<PortPublish>,
    pub volumes: Vec<VolumeMount>,
    #[serde(default)]
    pub binds: Vec<BindMount>,
    pub detached: bool,
    #[serde(default)]
    pub auto_remove: bool,
}

impl RunConfig {
    pub fn from_run_args(args: &RunImageArgs) -> Self {
        Self {
            cpus: args.cpus,
            mem_mib: args.mem,
            policy_path: args.policy_path.clone(),
            sandbox_user: args.sandbox_user.clone(),
            sandbox_uid: args.sandbox_uid,
            entrypoint: args.entrypoint.clone(),
            hostname: args.hostname.clone(),
            env: args.env.clone(),
            published_ports: args.published_ports.clone(),
            volumes: args.volumes.clone(),
            binds: args.binds.clone(),
            detached: args.detached,
            auto_remove: args.auto_remove,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusInfo {
    pub pid: u32,
    pub uptime_secs: u64,
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortPublish {
    pub host_ip: IpAddr,
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: Protocol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunImageArgs {
    pub image: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    pub cpus: u8,
    pub mem: usize,
    /// True when the user set `--cpus`/`-m` explicitly, so a bundle's Sandbox size can't silently override an explicit request that happens to equal the built-in default.
    #[serde(default)]
    pub cpus_explicit: bool,
    #[serde(default)]
    pub mem_explicit: bool,
    pub policy_path: Option<String>,
    #[serde(default)]
    pub sandbox_user: Option<String>,
    #[serde(default)]
    pub sandbox_uid: Option<u32>,
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
    pub cmd: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub workdir: Option<String>,
    pub debug: bool,
    #[serde(default = "default_tty")]
    pub tty: bool,
    #[serde(default = "default_stdin")]
    pub stdin: bool,
    #[serde(default)]
    pub initial_winsize: Option<(u16, u16)>,
    #[serde(default)]
    pub detached: bool,
    #[serde(default)]
    pub published_ports: Vec<PortPublish>,
    #[serde(default)]
    pub volumes: Vec<VolumeMount>,
    #[serde(default)]
    pub binds: Vec<BindMount>,
    #[serde(default)]
    pub auto_remove: bool,
    #[serde(default)]
    pub with: Vec<WithOverride>,
    #[serde(default)]
    pub insecure: bool,
    /// True when `image` is a reference the service must classify (refusing a plain OCI image that is not a sandbox); false for a local sandbox's base image, which the CLI has already resolved and the service runs directly.
    #[serde(default)]
    pub verify_sandbox: bool,
    /// A local sandbox definition as canonical JSON; the service plans it like a published sandbox so its policy, integrations, and resources apply.
    #[serde(default)]
    pub definition: Option<String>,
}

/// A launch-time `--with` component override, addressed by OCI reference; its mount path comes from the referenced FileSet's manifest at resolve time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithOverride {
    pub reference: String,
}

impl WithOverride {
    /// Parse a `--with <component-ref>` flag value.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let reference = spec.trim();
        if reference.is_empty() {
            return Err("invalid --with: empty component reference".to_string());
        }
        if reference.split_whitespace().count() != 1 {
            return Err(format!(
                "invalid --with {spec:?}: a component reference has no spaces"
            ));
        }
        Ok(Self {
            reference: reference.to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeMount {
    pub name: String,
    pub target: String,
    pub read_only: bool,
}

impl VolumeMount {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let body = spec
            .strip_suffix(":ro")
            .or_else(|| spec.strip_suffix(":rw"))
            .unwrap_or(spec);
        let (name, target) = body
            .split_once(':')
            .ok_or_else(|| format!("invalid volume {spec:?}: expected name:/path[:ro]"))?;
        validate_volume_name(name)?;
        validate_volume_target(target)?;
        Ok(Self {
            name: name.to_string(),
            target: target.to_string(),
            read_only: spec.ends_with(":ro"),
        })
    }
}

pub fn validate_volume_target(target: &str) -> Result<(), String> {
    if !target.starts_with('/') {
        return Err(format!(
            "invalid volume target {target:?}: must be an absolute path"
        ));
    }
    if target.chars().any(cmdline_unsafe_char) {
        let reason = "must not contain whitespace, quotes, or control characters";
        return Err(format!("invalid volume target {target:?}: {reason}"));
    }
    if target.split('/').any(|seg| seg == "." || seg == "..") {
        return Err(format!(
            "invalid volume target {target:?}: must not contain `.` or `..` path segments"
        ));
    }
    Ok(())
}

/// A char that can't safely ride the kernel cmdline value position the guest tokenizes (whitespace splits, `"` toggles quoting); rejected in volume targets, bind sources, and dropped-path names alike.
pub fn cmdline_unsafe_char(c: char) -> bool {
    c.is_whitespace() || c.is_control() || c == '"'
}

pub fn validate_volume_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("invalid volume name: must not be empty".to_string());
    }
    if name == "." || name == ".." {
        return Err(format!("invalid volume name {name:?}: reserved"));
    }
    match name.chars().find(|c| !name_char_allowed(*c)) {
        Some(bad) => Err(format!(
            "invalid volume name {name:?}: character {bad:?} not allowed"
        )),
        None => Ok(()),
    }
}

fn name_char_allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

pub fn validate_run_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("invalid run name: must not be empty".to_string());
    }
    if name
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(format!(
            "invalid run name {name:?}: a name must not be all lowercase hex (those address a run by id)"
        ));
    }
    match name.chars().find(|c| !name_char_allowed(*c)) {
        Some(bad) => Err(format!(
            "invalid run name {name:?}: character {bad:?} not allowed"
        )),
        None => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindMount {
    pub host_source: String,
    pub target: String,
    pub read_only: bool,
    #[serde(default)]
    pub dropped_paths: Vec<String>,
    /// Secret-shaped files the operator chose to expose; carried for the audit record, not consumed by the guest.
    #[serde(default)]
    pub kept_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindSpec {
    pub host_source: String,
    pub target: String,
    pub read_only: bool,
}

impl BindSpec {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let read_only = spec.ends_with(":ro");
        let body = spec
            .strip_suffix(":ro")
            .or_else(|| spec.strip_suffix(":rw"))
            .unwrap_or(spec);
        let (source, target) = body
            .split_once(':')
            .ok_or_else(|| format!("invalid host bind {spec:?}: expected /host-path:/path[:ro]"))?;
        validate_volume_target(target)?;
        validate_bind_source(source)?;
        Ok(Self {
            host_source: source.to_string(),
            target: target.to_string(),
            read_only,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountSpec {
    Named(VolumeMount),
    Bind(BindSpec),
}

impl MountSpec {
    pub fn parse(spec: &str) -> Result<Self, String> {
        if Self::source_is_path(spec) {
            return BindSpec::parse(spec).map(MountSpec::Bind);
        }
        VolumeMount::parse(spec).map(MountSpec::Named)
    }

    fn source_is_path(spec: &str) -> bool {
        let body = spec
            .strip_suffix(":ro")
            .or_else(|| spec.strip_suffix(":rw"))
            .unwrap_or(spec);
        let source = body.split_once(':').map_or(body, |(s, _)| s);
        source.starts_with('/')
    }

    pub fn target(&self) -> &str {
        match self {
            MountSpec::Named(v) => &v.target,
            MountSpec::Bind(b) => &b.target,
        }
    }
}

pub fn validate_bind_source(source: &str) -> Result<(), String> {
    if !source.starts_with('/') {
        return Err(format!(
            "invalid host bind source {source:?}: must be an absolute path"
        ));
    }
    if source.chars().any(cmdline_unsafe_char) {
        return Err(format!(
            "invalid host bind source {source:?}: must not contain whitespace, quotes, or control characters"
        ));
    }
    Ok(())
}

fn default_tty() -> bool {
    true
}

fn default_stdin() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecImageArgs {
    pub run: String,
    pub argv: Vec<String>,
    pub env: Vec<String>,
    #[serde(default = "default_tty")]
    pub tty: bool,
    #[serde(default = "default_stdin")]
    pub stdin: bool,
    #[serde(default)]
    pub initial_winsize: Option<(u16, u16)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_image_args_tty_defaults_to_true_when_missing() {
        let frame = serde_json::json!({
            "run": "1",
            "argv": ["sh"],
            "env": []
        });
        let parsed: ExecImageArgs = serde_json::from_value(frame).unwrap();
        assert!(parsed.tty);
        assert!(parsed.stdin);
    }

    #[test]
    fn run_image_args_env_defaults_to_empty_when_missing() {
        let frame = serde_json::json!({
            "image": "alpine",
            "cpus": 1,
            "mem": 512,
            "policy_path": null,
            "sandbox_user": "sandbox",
            "sandbox_uid": 65534,
            "cmd": [],
            "debug": false
        });
        let parsed: RunImageArgs = serde_json::from_value(frame).unwrap();
        assert!(parsed.env.is_empty());
    }

    #[test]
    fn run_image_args_published_ports_default_empty_when_missing() {
        let frame = serde_json::json!({
            "image": "prism",
            "cpus": 1,
            "mem": 512,
            "policy_path": null,
            "sandbox_user": "sandbox",
            "sandbox_uid": 65534,
            "cmd": [],
            "debug": false
        });
        let parsed: RunImageArgs = serde_json::from_value(frame).unwrap();
        assert!(parsed.published_ports.is_empty());
    }

    #[test]
    fn run_image_args_volumes_default_to_empty_when_missing() {
        let frame = serde_json::json!({
            "image": "ubuntu",
            "cpus": 1,
            "mem": 512,
            "policy_path": null,
            "sandbox_user": "sandbox",
            "sandbox_uid": 65534,
            "cmd": [],
            "debug": false
        });
        let parsed: RunImageArgs = serde_json::from_value(frame).unwrap();
        assert!(parsed.volumes.is_empty());
        assert_eq!(parsed.definition, None);
    }

    #[test]
    fn port_publish_survives_a_request_round_trip() {
        let mapping = PortPublish {
            host_ip: "127.0.0.1".parse().unwrap(),
            host_port: 3003,
            container_port: 3003,
            protocol: Protocol::Tcp,
        };
        let req = Request::RunImage(Box::new(RunImageArgs {
            image: Some("prism".into()),
            name: None,
            cpus: 1,
            mem: 512,
            cpus_explicit: false,
            mem_explicit: false,
            policy_path: None,
            sandbox_user: Some("sandbox".into()),
            sandbox_uid: Some(65534),
            entrypoint: None,
            hostname: None,
            cmd: Vec::new(),
            env: Vec::new(),
            workdir: None,
            debug: false,
            tty: true,
            stdin: true,
            initial_winsize: None,
            detached: false,
            published_ports: vec![mapping],
            volumes: Vec::new(),
            binds: Vec::new(),
            auto_remove: false,
            with: Vec::new(),
            insecure: false,
            verify_sandbox: false,
            definition: None,
        }));
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn run_image_args_volumes_and_binds_survive_postcard_round_trip() {
        let args = RunImageArgs {
            image: Some("ubuntu".into()),
            name: None,
            cpus: 1,
            mem: 512,
            cpus_explicit: false,
            mem_explicit: false,
            policy_path: None,
            sandbox_user: None,
            sandbox_uid: None,
            entrypoint: None,
            hostname: None,
            cmd: vec![],
            env: vec![],
            workdir: None,
            debug: false,
            tty: true,
            stdin: true,
            initial_winsize: None,
            detached: false,
            published_ports: vec![],
            volumes: vec![VolumeMount {
                name: "prism-data".into(),
                target: "/data".into(),
                read_only: true,
            }],
            binds: vec![BindMount {
                host_source: "/Users/me/proj".into(),
                target: "/work".into(),
                read_only: false,
                dropped_paths: vec![".env".into()],
                kept_paths: vec![".npmrc".into()],
            }],
            auto_remove: false,
            with: Vec::new(),
            insecure: false,
            verify_sandbox: false,
            definition: None,
        };
        let frame = crate::encode_frame(&args).unwrap();
        let decoded: RunImageArgs = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, args);
    }

    #[test]
    fn volume_requests_survive_request_round_trips() {
        for req in [
            Request::ListVolumes,
            Request::CreateVolume {
                name: "prism-data".into(),
            },
            Request::InspectVolume {
                name: "prism-data".into(),
            },
            Request::RemoveVolume {
                name: "prism-data".into(),
            },
            Request::PruneVolumes,
        ] {
            let frame = crate::encode_frame(&req).unwrap();
            let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
            assert_eq!(decoded, req);
        }
    }

    #[test]
    fn volume_responses_survive_round_trips() {
        let info = VolumeInfo {
            name: "prism-data".into(),
            size_bytes: 10 * 1024 * 1024 * 1024,
            disk_bytes: 32 * 1024 * 1024,
            created: "2026-06-10T12:00:00Z".into(),
            in_use_by: Some("1a2b3c4d0000000000000000000000aa".into()),
        };
        for resp in [
            Response::VolumeList {
                volumes: vec![info.clone()],
            },
            Response::VolumeCreated {
                volume: info.clone(),
            },
            Response::VolumeInspect {
                volume: info.clone(),
            },
            Response::VolumeRemoved {
                name: "prism-data".into(),
            },
            Response::VolumesPruned {
                removed: vec!["prism-data".into()],
                reclaimed_bytes: 32 * 1024 * 1024,
                failed: vec![VolumePruneFailure {
                    name: "scratch".into(),
                    error: "permission denied".into(),
                }],
            },
        ] {
            let frame = crate::encode_frame(&resp).unwrap();
            let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
            assert_eq!(decoded, resp);
        }
    }

    #[test]
    fn volume_info_serializes_idle_holder_as_null() {
        let info = VolumeInfo {
            name: "prism-data".into(),
            size_bytes: 4096,
            disk_bytes: 1024,
            created: "2026-06-10T12:00:00Z".into(),
            in_use_by: None,
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["in_use_by"], serde_json::Value::Null);
        assert_eq!(json["name"], "prism-data");
        assert_eq!(json["disk_bytes"], 1024);
    }

    #[test]
    fn stop_run_survives_a_request_round_trip() {
        let req = Request::StopRun {
            run: "7".into(),
            timeout_secs: 10,
        };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    fn sample_run_args() -> RunImageArgs {
        RunImageArgs {
            image: Some("some-image:1".into()),
            name: None,
            cpus: 2,
            mem: 1024,
            cpus_explicit: true,
            mem_explicit: true,
            policy_path: Some("/work/lns-policy.yaml".into()),
            sandbox_user: Some("sandbox".into()),
            sandbox_uid: Some(65534),
            entrypoint: Some("/bin/sh".into()),
            hostname: Some("demo".into()),
            cmd: vec!["echo".into(), "hi".into()],
            env: vec!["FOO=bar".into()],
            workdir: None,
            debug: false,
            tty: false,
            stdin: false,
            initial_winsize: None,
            detached: true,
            published_ports: vec![PortPublish {
                host_ip: "127.0.0.1".parse().unwrap(),
                host_port: 8080,
                container_port: 3003,
                protocol: Protocol::Tcp,
            }],
            volumes: vec![VolumeMount {
                name: "prism-data".into(),
                target: "/data".into(),
                read_only: false,
            }],
            binds: vec![BindMount {
                host_source: "/Users/me/proj".into(),
                target: "/work".into(),
                read_only: false,
                dropped_paths: vec![],
                kept_paths: vec![],
            }],
            auto_remove: true,
            with: Vec::new(),
            insecure: false,
            verify_sandbox: false,
            definition: Some(r#"{"kind":"Sandbox"}"#.into()),
        }
    }

    #[test]
    fn run_config_from_run_args_carries_the_launch_configuration() {
        let args = sample_run_args();
        let config = RunConfig::from_run_args(&args);
        assert_eq!(config.cpus, 2);
        assert_eq!(config.mem_mib, 1024);
        assert_eq!(config.policy_path.as_deref(), Some("/work/lns-policy.yaml"));
        assert_eq!(config.sandbox_user.as_deref(), Some("sandbox"));
        assert_eq!(config.entrypoint.as_deref(), Some("/bin/sh"));
        assert_eq!(config.hostname.as_deref(), Some("demo"));
        assert!(config.auto_remove);
        assert_eq!(config.sandbox_uid, Some(65534));
        assert_eq!(config.env, vec!["FOO=bar".to_string()]);
        assert_eq!(config.published_ports, args.published_ports);
        assert_eq!(config.volumes, args.volumes);
        assert_eq!(config.binds, args.binds);
        assert!(config.detached);
    }

    #[test]
    fn inspect_run_survives_a_request_round_trip() {
        let req = Request::InspectRun { run: "3".into() };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn run_logs_survives_a_request_round_trip() {
        for follow in [false, true] {
            let req = Request::RunLogs {
                run: "3".into(),
                follow,
            };
            let frame = crate::encode_frame(&req).unwrap();
            let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
            assert_eq!(decoded, req);
        }
    }

    #[test]
    fn attach_run_survives_a_request_round_trip() {
        let req = Request::AttachRun { run: "3".into() };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn run_stats_survives_a_request_and_response_round_trip() {
        let req = Request::RunStats { run: "3".into() };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);

        let resp = Response::RunStats {
            stats: RunStatsInfo {
                cpu_permille: 125,
                mem_used_bytes: 88 * 1024 * 1024,
                mem_total_bytes: 512 * 1024 * 1024,
            },
        };
        let frame = crate::encode_frame(&resp).unwrap();
        let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, resp);
    }

    #[test]
    fn run_inspect_survives_a_response_round_trip() {
        let resp = Response::RunInspect {
            details: Box::new(RunDetails {
                summary: RunSummary {
                    id: "1a2b3c4d0000000000000000000000aa".into(),
                    name: "reviewer".into(),
                    image: "some-image:1".into(),
                    command: "echo hi".into(),
                    status: RunStatus::Running,
                    started: "2026-01-01T00:00:00Z".into(),
                },
                config: RunConfig::from_run_args(&sample_run_args()),
            }),
        };
        let frame = crate::encode_frame(&resp).unwrap();
        let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, resp);
    }

    #[test]
    fn run_stopped_survives_a_response_round_trip() {
        for forced in [false, true] {
            let resp = Response::RunStopped { forced };
            let frame = crate::encode_frame(&resp).unwrap();
            let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
            assert_eq!(decoded, resp);
        }
    }

    #[test]
    fn run_progress_survives_round_trips_for_determinate_and_indeterminate() {
        for resp in [
            Response::RunProgress {
                verb: "Pulling".into(),
                message: String::new(),
                current: 12 * 1024 * 1024,
                total: 51 * 1024 * 1024,
            },
            Response::RunProgress {
                verb: "Booting".into(),
                message: "microVM".into(),
                current: 0,
                total: 0,
            },
        ] {
            let frame = crate::encode_frame(&resp).unwrap();
            let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
            assert_eq!(decoded, resp);
        }
    }

    #[test]
    fn remove_run_survives_a_request_round_trip() {
        let req = Request::RemoveRun { run: "7".into() };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn rename_run_survives_a_request_round_trip() {
        let req = Request::RenameRun {
            run: "reviewer".into(),
            new_name: "auditor".into(),
        };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn run_image_args_name_survives_a_round_trip_and_defaults_to_none() {
        let mut args = sample_run_args();
        args.name = Some("reviewer".into());
        let frame = crate::encode_frame(&args).unwrap();
        let decoded: RunImageArgs = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded.name.as_deref(), Some("reviewer"));

        let frame = serde_json::json!({
            "image": "alpine",
            "cpus": 1,
            "mem": 512,
            "policy_path": null,
            "cmd": [],
            "debug": false
        });
        let parsed: RunImageArgs = serde_json::from_value(frame).unwrap();
        assert_eq!(parsed.name, None);
    }

    #[test]
    fn prune_runs_survives_a_request_and_response_round_trip() {
        let req = Request::PruneRuns;
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);

        let resp = Response::RunsPruned {
            removed: vec![
                "1a2b3c4d0000000000000000000000aa".into(),
                "5e6f7a8b0000000000000000000000bb".into(),
            ],
        };
        let frame = crate::encode_frame(&resp).unwrap();
        let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, resp);
    }

    #[test]
    fn begin_integration_sign_in_survives_a_request_round_trip() {
        let req = Request::BeginIntegrationSignIn {
            id: "some-oauth".into(),
        };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn bind_integration_credential_survives_a_request_round_trip() {
        let req = Request::BindIntegrationCredential {
            id: "some-provider".into(),
        };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn credential_bind_responses_survive_round_trips() {
        for resp in [
            Response::CredentialBindComplete {
                decision: CredentialBindDecision::Stored,
            },
            Response::CredentialBindComplete {
                decision: CredentialBindDecision::HostDetect,
            },
            Response::CredentialBindComplete {
                decision: CredentialBindDecision::Denied,
            },
            Response::CredentialBindFailed {
                reason: "the value decision timed out".into(),
            },
        ] {
            let frame = crate::encode_frame(&resp).unwrap();
            let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
            assert_eq!(decoded, resp);
        }
    }

    #[test]
    fn run_detach_request_survives_a_round_trip() {
        let req = Request::RunDetach {
            run_id: "7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a".to_string(),
        };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn detach_accepted_response_survives_a_round_trip() {
        let resp = Response::DetachAccepted;
        let frame = crate::encode_frame(&resp).unwrap();
        let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, resp);
    }

    #[test]
    fn oauth_sign_in_responses_survive_round_trips() {
        for resp in [
            Response::OauthVerification {
                verification_uri: "https://example.com/login/device".into(),
                user_code: "WDJB-MJHT".into(),
                expires_in_secs: 900,
            },
            Response::OauthBrowserOpened {
                authorization_url: "https://example.com/auth?code_challenge=abc".into(),
            },
            Response::OauthSignInComplete,
            Response::OauthSignInFailed {
                reason: "access_denied".into(),
            },
        ] {
            let frame = crate::encode_frame(&resp).unwrap();
            let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
            assert_eq!(decoded, resp);
        }
    }

    #[test]
    fn image_requests_survive_round_trips() {
        for req in [
            Request::PullImage {
                image: "registry.example.test/some/image:1.0".into(),
            },
            Request::ListImages,
            Request::RemoveImage {
                image: "registry.example.test/some/image:1.0".into(),
            },
            Request::PruneImages,
        ] {
            let frame = crate::encode_frame(&req).unwrap();
            let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
            assert_eq!(decoded, req);
        }
    }

    #[test]
    fn image_responses_survive_round_trips() {
        let info = ImageInfo {
            reference: "registry.example.test/some/image:1.0".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            size_bytes: 3 * 1024 * 1024,
            layers: 2,
            pulled: "2026-06-10T12:00:00Z".into(),
            in_use_by: Some("1a2b3c4d0000000000000000000000aa".into()),
        };
        for resp in [
            Response::ImagePulled {
                image: info.clone(),
            },
            Response::ImageList {
                images: vec![info.clone()],
            },
            Response::ImageRemoved {
                reference: info.reference.clone(),
                reclaimed_bytes: 3 * 1024 * 1024,
            },
            Response::ImagesPruned {
                removed: vec![info.reference.clone()],
                reclaimed_bytes: 3 * 1024 * 1024,
            },
        ] {
            let frame = crate::encode_frame(&resp).unwrap();
            let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
            assert_eq!(decoded, resp);
        }
    }

    #[test]
    fn registry_login_request_survives_a_round_trip() {
        let req = Request::RegistryLogin {
            registry: "ghcr.io".into(),
            username: "octocat".into(),
            secret: "ghp_real_token".into(),
        };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn registry_login_verified_response_survives_a_round_trip() {
        let resp = Response::RegistryLoginVerified;
        let frame = crate::encode_frame(&resp).unwrap();
        let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, resp);
    }

    #[test]
    fn image_info_serializes_idle_holder_as_null() {
        let info = ImageInfo {
            reference: "registry.example.test/some/image:1.0".into(),
            digest: format!("sha256:{}", "b".repeat(64)),
            size_bytes: 4096,
            layers: 1,
            pulled: "2026-06-10T12:00:00Z".into(),
            in_use_by: None,
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["in_use_by"], serde_json::Value::Null);
        assert_eq!(json["reference"], "registry.example.test/some/image:1.0");
        assert_eq!(json["size_bytes"], 4096);
        assert_eq!(json["layers"], 1);
    }

    #[test]
    fn volume_mount_parse_name_and_absolute_target_defaults_to_rw() {
        let v = VolumeMount::parse("prism-data:/data").unwrap();
        assert_eq!(
            v,
            VolumeMount {
                name: "prism-data".into(),
                target: "/data".into(),
                read_only: false,
            }
        );
    }

    #[test]
    fn volume_mount_parse_honors_ro_and_rw_suffixes() {
        assert!(VolumeMount::parse("v:/data:ro").unwrap().read_only);
        assert!(!VolumeMount::parse("v:/data:rw").unwrap().read_only);
    }

    #[test]
    fn with_override_parse_takes_a_component_reference() {
        assert_eq!(
            WithOverride::parse("some-registry.example/skills/deep@sha256:abcd").unwrap(),
            WithOverride {
                reference: "some-registry.example/skills/deep@sha256:abcd".into(),
            }
        );
    }

    #[test]
    fn with_override_parse_rejects_an_empty_or_spaced_reference() {
        assert!(WithOverride::parse("   ").unwrap_err().contains("empty"));
        assert!(
            WithOverride::parse("reg/a reg/b")
                .unwrap_err()
                .contains("no spaces")
        );
    }

    #[test]
    fn volume_mount_parse_rejects_missing_target_separator() {
        VolumeMount::parse("prism-data").unwrap_err();
    }

    #[test]
    fn volume_mount_parse_rejects_relative_target() {
        VolumeMount::parse("v:data").unwrap_err();
    }

    #[test]
    fn volume_mount_parse_rejects_invalid_name_with_path_separator() {
        let err = VolumeMount::parse("../etc:/data").unwrap_err();
        assert!(err.contains("invalid volume name"), "got: {err}");
    }

    #[test]
    fn volume_mount_parse_rejects_target_that_would_inject_kernel_cmdline_tokens() {
        let err = VolumeMount::parse("data:/d quiet console=ttyS0").unwrap_err();
        assert!(err.contains("must not contain whitespace"), "got: {err}");
        VolumeMount::parse("data:/my data").unwrap_err();
        VolumeMount::parse("data:/d\tx").unwrap_err();
        VolumeMount::parse("data:/d\nx").unwrap_err();
    }

    #[test]
    fn volume_mount_parse_rejects_target_with_a_double_quote() {
        let err = VolumeMount::parse(r#"data:/d"x"#).unwrap_err();
        assert!(err.contains("must not contain whitespace"), "got: {err}");
    }

    #[test]
    fn validate_run_name_accepts_the_volume_charset() {
        for ok in ["reviewer", "build-cache", "agent.v2_3", "g", "x7"] {
            validate_run_name(ok).unwrap();
        }
    }

    #[test]
    fn validate_run_name_rejects_all_hex_names_so_ids_stay_unambiguous() {
        let err = validate_run_name("7").unwrap_err();
        assert!(err.contains("all lowercase hex"), "got: {err}");
        validate_run_name("0042").unwrap_err();
        validate_run_name("abcdef").unwrap_err();
        validate_run_name("1a2b3c4d").unwrap_err();
    }

    #[test]
    fn validate_run_name_rejects_empty_and_illegal_characters() {
        validate_run_name("").unwrap_err();
        let err = validate_run_name("has space").unwrap_err();
        assert!(err.contains("not allowed"), "got: {err}");
        validate_run_name("a/b").unwrap_err();
        validate_run_name("a:b").unwrap_err();
    }

    #[test]
    fn validate_volume_name_rejects_empty_dots_and_separators() {
        for bad in ["", ".", "..", "a/b", "a:b"] {
            validate_volume_name(bad).unwrap_err();
        }
        validate_volume_name("prism-data.v2_3").unwrap();
    }

    #[test]
    fn volume_mount_parse_rejects_dot_dot_segments_that_escape_the_overlay_root() {
        for bad in [
            "data:/../../proc",
            "data:/data/../../etc",
            "data:/..",
            "data:/foo/..",
        ] {
            let err = VolumeMount::parse(bad).unwrap_err();
            assert!(err.contains("must not contain"), "{bad}: got {err}");
        }
    }

    #[test]
    fn validate_volume_target_accepts_a_dot_inside_a_filename_but_rejects_dot_segments() {
        validate_volume_target("/srv/app.data").unwrap();
        validate_volume_target("/srv/.config").unwrap();
        validate_volume_target("/srv/..").unwrap_err();
        validate_volume_target("/../etc").unwrap_err();
        validate_volume_target("/a/./b").unwrap_err();
    }

    #[test]
    fn mount_spec_parse_routes_an_absolute_source_to_a_host_bind() {
        let m = MountSpec::parse("/Users/me/proj:/work").unwrap();
        assert_eq!(
            m,
            MountSpec::Bind(BindSpec {
                host_source: "/Users/me/proj".into(),
                target: "/work".into(),
                read_only: false,
            })
        );
    }

    #[test]
    fn mount_spec_parse_routes_a_bare_name_to_a_named_volume() {
        let m = MountSpec::parse("build-cache:/cache").unwrap();
        assert_eq!(
            m,
            MountSpec::Named(VolumeMount {
                name: "build-cache".into(),
                target: "/cache".into(),
                read_only: false,
            })
        );
    }

    #[test]
    fn mount_spec_target_reads_through_either_variant() {
        assert_eq!(MountSpec::parse("/h:/work").unwrap().target(), "/work");
        assert_eq!(MountSpec::parse("vol:/data").unwrap().target(), "/data");
    }

    #[test]
    fn bind_spec_parse_honors_ro_and_rw_suffixes() {
        assert!(BindSpec::parse("/h:/work:ro").unwrap().read_only);
        assert!(!BindSpec::parse("/h:/work:rw").unwrap().read_only);
        assert!(!BindSpec::parse("/h:/work").unwrap().read_only);
    }

    #[test]
    fn bind_spec_parse_rejects_a_relative_target() {
        let err = BindSpec::parse("/Users/me/proj:work").unwrap_err();
        assert!(err.contains("must be an absolute path"), "got: {err}");
    }

    #[test]
    fn bind_spec_parse_rejects_a_missing_target_separator() {
        BindSpec::parse("/Users/me/proj").unwrap_err();
    }

    #[test]
    fn a_relative_dot_dot_source_stays_a_named_volume_and_keeps_its_error() {
        let err = MountSpec::parse("../etc:/data").unwrap_err();
        assert!(err.contains("invalid volume name"), "got: {err}");
    }

    #[test]
    fn validate_bind_source_requires_absolute_and_clean_chars() {
        validate_bind_source("/Users/me/proj").unwrap();
        assert!(
            validate_bind_source("relative/path")
                .unwrap_err()
                .contains("must be an absolute path")
        );
        assert!(
            validate_bind_source("/has a space")
                .unwrap_err()
                .contains("must not contain whitespace")
        );
    }
}
