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
    BeginConnectorSignIn {
        id: String,
    },
    BindConnectorCredential {
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
        expected_digest: String,
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
        warnings: Vec<String>,
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
    Sandbox(Box<SandboxView>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageView {
    pub reference: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxView {
    pub reference: String,
    #[serde(default)]
    pub digest: String,
    pub image: String,
    #[serde(default)]
    pub workdir: Option<String>,
    /// The run-as user the sandbox declared, so a pulled artifact asking for root is visible before it boots.
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub mounts: Vec<SandboxMount>,
    #[serde(default)]
    pub ports: Vec<SandboxPort>,
    #[serde(default)]
    pub filesets: Vec<SandboxFileset>,
    #[serde(default)]
    pub connectors: Vec<String>,
    #[serde(default)]
    pub host_access: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub credentials: Vec<SandboxCredential>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub policy_flags: Vec<String>,
    /// What the sandbox declared, not what it resolves to — `None` must stay distinguishable from "declared exactly the default", or a flag can no longer outrank a declaration.
    #[serde(default)]
    pub cpus: Option<u8>,
    #[serde(default)]
    pub mem_mib: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxCredential {
    pub name: String,
    pub env: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxFileset {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(rename = "ref", default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub inline: bool,
    pub mount_path: String,
    #[serde(default)]
    pub owner: SandboxFilesetOwner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SandboxFilesetOwner {
    #[default]
    Workload,
    Root,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxPort {
    #[serde(default)]
    pub host: Option<u16>,
    pub container: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMountKind {
    Bind,
    Volume,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxMount {
    pub kind: SandboxMountKind,
    pub source: String,
    pub target: String,
    pub read_only: bool,
    /// Subpaths a published bind excluded, so a pulled sandbox's declared exclusions are visible and applied.
    #[serde(default)]
    pub exclude: Vec<String>,
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
    #[serde(default)]
    pub workdir: Option<String>,
    pub published_ports: Vec<PortPublish>,
    pub volumes: Vec<VolumeMount>,
    #[serde(default)]
    pub binds: Vec<BindMount>,
    #[serde(default)]
    pub socket_forwards: Vec<SocketForward>,
    #[serde(default)]
    pub host_access: Vec<HostAccessGrant>,
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
            workdir: args.workdir.clone(),
            published_ports: args.published_ports.clone(),
            volumes: args.volumes.clone(),
            binds: args.binds.clone(),
            socket_forwards: args.socket_forwards.clone(),
            host_access: args.host_access.clone(),
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

/// One host unix socket the guest may reach, proxied over vsock. The guest creates `guest_target` and every connection to it is relayed to `host_source`; the socket itself is never shared, because virtiofs passes the inode rather than the endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocketForward {
    pub id: String,
    pub host_source: String,
    /// Absolute, or `~/`-relative to the run-as user's home, which only the guest can resolve.
    pub guest_target: String,
}

impl SocketForward {
    /// Each field reaches a line-oriented guest manifest, so the same chokepoint rules as a bind source apply, plus a ban on the newline that would forge a second entry.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() || self.id.chars().any(cmdline_unsafe_char) {
            return Err(format!(
                "invalid socket forward id {:?}: must be non-empty and free of whitespace, quotes, and control characters",
                self.id
            ));
        }
        validate_bind_source(&self.host_source)
            .map_err(|e| e.replace("host bind source", "socket forward source"))?;
        if !self.guest_target.starts_with('/') && !self.guest_target.starts_with("~/") {
            return Err(format!(
                "invalid socket forward target {:?}: must be absolute or start with ~/",
                self.guest_target
            ));
        }
        if self.guest_target.chars().any(cmdline_unsafe_char)
            || self.guest_target.split('/').any(|seg| seg == "..")
        {
            return Err(format!(
                "invalid socket forward target {:?}: must not contain whitespace, quotes, control characters, or a .. segment",
                self.guest_target
            ));
        }
        Ok(())
    }
}

/// A directory the guest creates in the run-as user's home before any projected file lands in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAccessDir {
    pub target: String,
    pub mode: u32,
}

/// Bytes the host resolved (a filtered git config, a public keyring) for the guest to write into the run-as user's home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAccessFile {
    pub target: String,
    pub mode: u32,
    #[serde(with = "crate::base64_bytes")]
    pub contents: Vec<u8>,
}

/// Everything one granted host capability puts in the guest: the directories it needs, the files it projects, and the socket the guest creates for its forward.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAccessGrant {
    pub id: String,
    #[serde(default)]
    pub socket_target: Option<String>,
    #[serde(default)]
    pub dirs: Vec<HostAccessDir>,
    #[serde(default)]
    pub files: Vec<HostAccessFile>,
}

impl HostAccessGrant {
    /// Same chokepoint as a socket forward: every target reaches a line-oriented guest manifest, so a newline or a `..` segment must never make it through.
    pub fn validate(&self) -> Result<(), String> {
        for target in self
            .socket_target
            .iter()
            .chain(self.dirs.iter().map(|d| &d.target))
            .chain(self.files.iter().map(|f| &f.target))
        {
            validate_guest_target(target)?;
        }
        Ok(())
    }
}

pub fn validate_guest_target(target: &str) -> Result<(), String> {
    if !target.starts_with('/') && !target.starts_with("~/") {
        return Err(format!(
            "invalid host access target {target:?}: must be absolute or start with ~/"
        ));
    }
    if target.chars().any(cmdline_unsafe_char) || target.split('/').any(|seg| seg == "..") {
        return Err(format!(
            "invalid host access target {target:?}: must not contain whitespace, quotes, control characters, or a .. segment"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunImageArgs {
    pub image: Option<String>,
    #[serde(default)]
    pub resolved_image: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    pub cpus: u8,
    pub mem: usize,
    /// True when the user set `--cpus`/`-m` explicitly, so a published sandbox's size can't silently override an explicit request that happens to equal the built-in default.
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
    pub socket_forwards: Vec<SocketForward>,
    #[serde(default)]
    pub host_access: Vec<HostAccessGrant>,
    #[serde(default)]
    pub auto_remove: bool,
    /// True when `image` is a reference the service must classify (refusing a plain OCI image that is not a sandbox); false for a local sandbox's base image, which the CLI has already resolved and the service runs directly.
    #[serde(default)]
    pub verify_sandbox: bool,
    /// A local sandbox definition as canonical JSON; the service plans it like a published sandbox so its policy, connectors, and resources apply.
    #[serde(default)]
    pub definition: Option<String>,
    /// The local definition's absolute directory, keying its per-workload connector grants; absent for a published reference (which keys by repo@digest instead).
    #[serde(default)]
    pub definition_dir: Option<String>,
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
    // The guest hands the broker its mounted volume targets as one colon-separated list, and a target it cannot name is a volume nothing releases.
    if target.contains(':') {
        return Err(format!(
            "invalid volume target {target:?}: must not contain `:`"
        ));
    }
    if target.split('/').any(|seg| seg == "." || seg == "..") {
        return Err(format!(
            "invalid volume target {target:?}: must not contain `.` or `..` path segments"
        ));
    }
    if shadows_runtime_namespace(target) {
        return Err(format!(
            "invalid volume target {target:?}: must not overlap the /.lens runtime namespace, which belongs to the sandbox itself"
        ));
    }
    Ok(())
}

/// True for `/` and any path whose first real segment is `.lens`; the runtime tree (supervisor, guest tools) can never be shadowed by a mount, whatever its source.
fn shadows_runtime_namespace(target: &str) -> bool {
    match target.split('/').find(|seg| !seg.is_empty() && *seg != ".") {
        None => true,
        Some(first) => first == ".lens",
    }
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
    /// Subpaths the definition excluded, unioned with any `.lensignore` before the secret scan.
    pub exclude: Vec<String>,
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
            // A `-v` carries no exclude; the definition is the author surface for that.
            exclude: Vec::new(),
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
            host_access: Vec::new(),
            image: Some("prism".into()),
            resolved_image: None,
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
            socket_forwards: Vec::new(),
            auto_remove: false,
            verify_sandbox: false,
            definition: None,
            definition_dir: None,
        }));
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn run_image_args_declarative_launch_settings_survive_postcard_round_trip() {
        let args = RunImageArgs {
            host_access: Vec::new(),
            socket_forwards: Vec::new(),
            image: Some("ubuntu".into()),
            resolved_image: Some(format!("ubuntu@sha256:{}", "a".repeat(64))),
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
            workdir: Some("/workspace".into()),
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
            verify_sandbox: false,
            definition: None,
            definition_dir: None,
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
            host_access: Vec::new(),
            socket_forwards: Vec::new(),
            image: Some("some-image:1".into()),
            resolved_image: None,
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
            workdir: Some("/workspace".into()),
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
            verify_sandbox: false,
            definition: Some(r#"{"kind":"Sandbox"}"#.into()),
            definition_dir: Some("/work/proj".into()),
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
        assert_eq!(config.workdir.as_deref(), Some("/workspace"));
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
    fn begin_connector_sign_in_survives_a_request_round_trip() {
        let req = Request::BeginConnectorSignIn {
            id: "some-oauth".into(),
        };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn bind_connector_credential_survives_a_request_round_trip() {
        let req = Request::BindConnectorCredential {
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
                expected_digest: format!("sha256:{}", "a".repeat(64)),
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
                warnings: vec!["the first run needs the network".into()],
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

    fn forward(host_source: &str, guest_target: &str) -> SocketForward {
        SocketForward {
            id: "some-access".into(),
            host_source: host_source.into(),
            guest_target: guest_target.into(),
        }
    }

    fn grant_with(target: &str) -> HostAccessGrant {
        HostAccessGrant {
            id: "some-access".into(),
            socket_target: Some("~/.gnupg/S.gpg-agent".into()),
            dirs: vec![HostAccessDir {
                target: "~/.gnupg".into(),
                mode: 0o700,
            }],
            files: vec![HostAccessFile {
                target: target.into(),
                mode: 0o600,
                contents: b"body".to_vec(),
            }],
        }
    }

    #[test]
    fn a_well_formed_grant_validates_every_target_it_carries() {
        grant_with("~/.gitconfig")
            .validate()
            .expect("should validate");
        grant_with("/etc/lns/config")
            .validate()
            .expect("an absolute target is fine");
    }

    #[test]
    fn a_grant_target_must_be_rooted() {
        let err = grant_with("relative/config").validate().unwrap_err();
        assert!(err.contains("must be absolute or start with ~/"), "{err}");
    }

    #[test]
    fn a_grant_target_may_not_escape_the_home_or_forge_a_manifest_line() {
        for bad in [
            "~/../escape",
            "~/ok/../bad",
            "~/with space",
            "~/with\nnewline",
        ] {
            let err = grant_with(bad).validate().unwrap_err();
            assert!(err.contains("host access target"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn a_grant_validates_its_socket_and_directory_targets_too() {
        let mut socket = grant_with("~/.gitconfig");
        socket.socket_target = Some("~/../escape".into());
        assert!(socket.validate().is_err());
        let mut dir = grant_with("~/.gitconfig");
        dir.dirs[0].target = "relative".into();
        assert!(dir.validate().is_err());
    }

    #[test]
    fn a_grant_with_no_targets_at_all_validates() {
        let empty = HostAccessGrant {
            id: "some-access".into(),
            socket_target: None,
            dirs: Vec::new(),
            files: Vec::new(),
        };
        empty.validate().expect("nothing to check is not a failure");
    }

    #[test]
    fn projected_bytes_survive_the_json_frame_as_base64() {
        let file = HostAccessFile {
            target: "~/.gnupg/pubring.kbx".into(),
            mode: 0o600,
            // A keyring is binary and includes bytes JSON cannot carry as text.
            contents: (0..=255u8).collect(),
        };
        let json = serde_json::to_string(&file).unwrap();
        assert!(
            json.contains("\"contents\":\""),
            "contents must be one base64 string, not a byte list: {json}"
        );
        let parsed: HostAccessFile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, file);
    }

    #[test]
    fn a_frame_whose_contents_are_not_base64_is_refused() {
        let json = r#"{"target":"~/x","mode":384,"contents":"not base64!"}"#;
        let parsed: serde_json::Result<HostAccessFile> = serde_json::from_str(json);
        assert!(parsed.is_err());
    }

    #[test]
    fn a_well_formed_socket_forward_validates() {
        forward("/run/user/501/agent.sock", "~/.gnupg/S.gpg-agent")
            .validate()
            .expect("should validate");
        forward("/run/user/501/agent.sock", "/run/lns/agent.sock")
            .validate()
            .expect("an absolute target is fine");
    }

    #[test]
    fn a_socket_forward_source_must_be_an_absolute_cmdline_safe_path() {
        for bad in [
            "relative/agent.sock",
            "/run/with space.sock",
            "/run/quote\".sock",
        ] {
            let err = forward(bad, "~/.gnupg/S.gpg-agent").validate().unwrap_err();
            assert!(err.contains("socket forward source"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn a_socket_forward_target_must_be_rooted_and_carry_no_parent_segment() {
        for bad in [
            "relative/sock",
            "~/../escape/sock",
            "~/ok/../bad",
            "~/with space",
            "~/with\nnewline",
        ] {
            let err = forward("/run/agent.sock", bad).validate().unwrap_err();
            assert!(err.contains("socket forward target"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn a_socket_forward_id_must_be_non_empty_and_cmdline_safe() {
        for bad in ["", "two words", "quote\""] {
            let mut f = forward("/run/agent.sock", "~/x");
            f.id = bad.into();
            let err = f.validate().unwrap_err();
            assert!(err.contains("socket forward id"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn socket_forwards_default_to_empty_so_an_older_cli_still_deserializes() {
        let json = serde_json::to_value(sample_run_args()).unwrap();
        let mut object = json.as_object().unwrap().clone();
        object.remove("socket_forwards");
        let parsed: RunImageArgs =
            serde_json::from_value(serde_json::Value::Object(object)).unwrap();
        assert!(parsed.socket_forwards.is_empty());
    }

    #[test]
    fn sandbox_view_round_trips_declarative_launch_settings() {
        let view = SandboxView {
            reference: "registry.example.test/team/sandbox:1".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            image: "registry.example.test/runtime:1".into(),
            workdir: Some("/workspace".into()),
            user: None,
            mounts: vec![SandboxMount {
                kind: SandboxMountKind::Bind,
                source: ".".into(),
                target: "/workspace".into(),
                read_only: true,
                exclude: Vec::new(),
            }],
            ports: vec![
                SandboxPort {
                    host: None,
                    container: 3003,
                },
                SandboxPort {
                    host: Some(8080),
                    container: 9090,
                },
            ],
            filesets: vec![SandboxFileset {
                path: None,
                reference: Some(format!(
                    "registry.example.test/team/skills@sha256:{}",
                    "a".repeat(64)
                )),
                inline: false,
                mount_path: "/root/.agent/skills".into(),
                owner: SandboxFilesetOwner::Workload,
            }],
            connectors: Vec::new(),
            host_access: Vec::new(),
            env: vec!["SHELL=/bin/sh".into()],
            credentials: vec![SandboxCredential {
                name: "some-provider".into(),
                env: "SOME_TOKEN".into(),
                required: true,
            }],
            tools: vec!["node@22.11.0".into()],
            policy_flags: Vec::new(),
            cpus: Some(3),
            mem_mib: Some(6144),
        };
        let response = Response::ImageInspected {
            inspection: ArtifactInspection::Sandbox(Box::new(view)),
        };
        let frame = crate::encode_frame(&response).unwrap();
        let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, response);
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
    fn validate_volume_target_refuses_a_colon_so_the_guest_can_name_it_for_release() {
        let err = validate_volume_target("/da:ta").unwrap_err();
        assert!(err.contains("must not contain `:`"), "got: {err}");
    }

    #[test]
    fn validate_volume_target_refuses_shadowing_the_lens_runtime_namespace() {
        for target in ["/", "/.lens", "/.lens/guest-tools/bin"] {
            let err = validate_volume_target(target).unwrap_err();
            assert!(
                err.contains("/.lens runtime namespace"),
                "a -v mount over {target:?} must be refused: {err}"
            );
        }
        validate_volume_target("/.lensfoo").unwrap();
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
                exclude: Vec::new(),
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
