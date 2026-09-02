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
    SessionStdin {
        target: SessionTarget,
        bytes: Vec<u8>,
    },
    SessionStdinClose {
        target: SessionTarget,
    },
    SessionResize {
        target: SessionTarget,
        rows: u16,
        cols: u16,
    },
    SessionSignal {
        target: SessionTarget,
        signal: SignalKind,
    },
    SessionDetach {
        target: SessionTarget,
    },
    ExecImage(ExecImageArgs),
    Kill {
        run: String,
        signal: SignalKind,
    },
    ListRuns,
    StartRun {
        run: String,
        attach: bool,
        stdin: bool,
    },
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
        force: bool,
    },
    /// Render one run as a document the user keeps; the service owns the run's state, and the caller owns the file it lands in.
    SaveRun {
        run: String,
        kind: SaveKind,
        /// The `name` the rendered document carries, which the caller takes from the file it will write.
        name: String,
    },
    PruneRuns,
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
    /// The references `PruneImages` would remove right now, so a prune can list them and ask first.
    ListPrunableImages,
    /// Resolve a local definition's mixins, since only the service can pull a reference and read a directory the same way a run will.
    ResolveDefinition {
        definition: String,
        project_dir: String,
        #[serde(default)]
        mixins: Vec<String>,
    },
    InspectImage {
        image: String,
        /// The mixin references the user named on the command line, in flag order, so the preflight describes the document that will boot.
        #[serde(default)]
        mixins: Vec<String>,
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
    RegistryLogout {
        registry: String,
    },
    ListRegistryLogins,
    /// `<REF|PATH>` as the user typed it, absolutised by `lns` when it is a path — only the service can reach a registry, and a connector's `path` filesets arrive as layers of its own artifact.
    InstallConnector {
        source: String,
    },
    UninstallConnector {
        name: String,
    },
    ListConnectors,
    /// The values an authentication returned, keyed by the credential they belong to. The caller collects them because only it has a terminal to ask at.
    ConnectConnector {
        name: String,
        method: String,
        connection: String,
        values: SecretValues,
    },
    DisconnectConnector {
        name: String,
        connection: Option<String>,
    },
    /// `run` is the handle the user typed: only the service holds the registry that resolves it.
    GrantConnector {
        name: String,
        run: String,
        method: String,
        connection: Option<String>,
    },
    ForgetConnector {
        name: String,
        run: String,
    },
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
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionTarget {
    Primary { run_id: String },
    Exec { run_id: String, session_id: String },
}

impl SessionTarget {
    pub fn run_id(&self) -> &str {
        match self {
            Self::Primary { run_id } | Self::Exec { run_id, .. } => run_id,
        }
    }
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
    ExecStarted {
        run_id: String,
        session_id: String,
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
    /// No run answers to the handle — distinct from `Error` so a caller can treat a miss as an answer, never as a fault.
    RunUnknown {
        run: String,
    },
    RunStats {
        stats: RunStatsInfo,
    },
    RunSaved {
        document: String,
    },
    RunsPruned {
        removed: Vec<String>,
    },
    RegistryLoginStored,
    RegistryLoggedOut,
    RegistryLogins {
        logins: Vec<RegistryLoginSummary>,
    },
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
    /// A pulled mixin has no index entry: nothing runs it and nothing has to reclaim it, so the answer is what was cached rather than a row.
    MixinPulled {
        reference: String,
        digest: String,
        cached_mixins: usize,
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
    DefinitionResolved {
        /// The merged document, which declares no mixins of its own.
        definition: String,
        /// Every source the merge reached, as the disclosure names them.
        mixins: Vec<String>,
        /// What each mixin the user named resolved to, in the order they named them.
        pinned_mixins: Vec<String>,
        /// Which source decided each entry of the merged document, so the disclosure can attribute every line it shows.
        #[serde(default)]
        contributions: Vec<SourceContribution>,
        /// The egress every document source decided, as JSON: the run folds the live decisions file over it rather than over the copy the merged document carries, so a rule deleted mid-run retracts.
        #[serde(default)]
        authored_egress: String,
        /// Which artifact carries each packed fileset the merge reached; the merged document alone cannot say, since a mixin's entry is a layer of the mixin's own artifact.
        #[serde(default)]
        packed_filesets: Vec<PackedFilesetSource>,
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
    ConnectorInstalled {
        connector: ConnectorView,
    },
    /// Uninstalling drops connections and keeps grants, so it reports both rather than a bare acknowledgement.
    ConnectorUninstalled {
        name: String,
        dropped_connections: usize,
    },
    ConnectorList {
        connectors: Vec<ConnectorView>,
    },
    /// No connector answers to the name — an answer, never a fault, the way `RunUnknown` is.
    ConnectorUnknown {
        name: String,
    },
    /// `invalidated` names the project directories whose grant the returned authority no longer matches, so each is asked again.
    ConnectorConnected {
        name: String,
        connection: String,
        invalidated: Vec<String>,
    },
    ConnectorDisconnected {
        name: String,
        dropped: usize,
    },
    /// `displaced` names the method this grant replaced, since a project holds one grant per connector; `unchanged` means the project already held exactly this grant.
    ConnectorGranted {
        name: String,
        method: String,
        connection: Option<String>,
        displaced: Option<String>,
        unchanged: bool,
        /// Which row this wrote. The service decides it, because a run may appear between the caller's probe and the write.
        reserved: bool,
    },
    ConnectorForgotten {
        name: String,
        had_decision: bool,
        /// Whether what it cleared was a reservation rather than a run's own decision.
        reserved: bool,
    },
}

/// Real credential values, wrapped so no `Debug` of a request can put one in a log or a panic message.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct SecretValues(pub std::collections::BTreeMap<String, String>);

impl std::fmt::Debug for SecretValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{} redacted>", self.0.len())
    }
}

/// One installed connector as `lns connector list` shows it: what it serves, how it can be connected, and the connections this machine holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorView {
    pub name: String,
    pub digest: String,
    pub serves: Vec<String>,
    pub methods: Vec<ConnectorMethodView>,
    pub connections: Vec<ConnectorConnectionView>,
}

/// One method, with the whole payload a grant applies — a card that showed less would take consent for what it did not disclose (sandbox-spec §1.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorMethodView {
    pub name: String,
    pub label: String,
    /// What a connect calls the value it asks for — the `auth`'s own label, else its kind. Absent for a method with no `auth`: there is nothing to connect, so it is granted directly.
    pub auth_label: Option<String>,
    /// False where this version does not implement the method's `auth.kind`, which the card must not offer.
    pub offerable: bool,
    /// The egress this method opens, which `serves` does not bound.
    pub opens: Vec<String>,
    /// The guest paths its filesets write.
    pub writes: Vec<String>,
    /// The plain `env` keys it sets.
    pub env: Vec<String>,
    /// Each credential it sets, named by its `envVar` or, absent one, its placeholder.
    pub credentials: Vec<String>,
    /// Each `auth` output a connect must supply a value for, deduplicated: two credentials drawing on one output are one value to ask for, and it is this key the grant reads the value back under (§4.1).
    pub asks: Vec<String>,
    /// The connector author's own words about where to get the value (§3.2.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// What this method opens that the sandbox document denies (§3.2.4), or `None` where no document was in hand, so "nothing is overridden" and "nobody could check" never render alike.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<Vec<String>>,
}

impl ConnectorMethodView {
    /// Every variable applying this method sets, which §3.2.4 requires the card and `grant` to name: its plain `env`, and the variable each credential fills.
    pub fn sets(&self) -> Vec<String> {
        self.env
            .iter()
            .chain(self.credentials.iter())
            .cloned()
            .collect()
    }
}

impl ConnectorView {
    /// A name for a new connection that this connector does not already hold: the method's own name, then `-2`, `-3`. A label it holds would replace that account rather than add one, and the store keys a connection by connector and label alone — so a name another method made still collides. Both the CLI and the approval card name connections by this rule, and they must agree.
    pub fn free_connection_name(&self, method: &str) -> String {
        let mut candidate = method.to_string();
        let mut nth = 1;
        while self.connections.iter().any(|held| held.label == candidate) {
            nth += 1;
            candidate = format!("{method}-{nth}");
        }
        candidate
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorConnectionView {
    pub label: String,
    pub method: String,
    pub authority: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryLoginSummary {
    pub registry: String,
    pub username: String,
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

/// What a cached artifact is: an `lns.run/v1` sandbox or mixin document. The base images sandboxes run on stay internal to the cache and take no kind here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CachedKind {
    Sandbox,
    Mixin,
}

impl CachedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CachedKind::Sandbox => "sandbox",
            CachedKind::Mixin => "mixin",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageInfo {
    pub reference: String,
    pub kind: CachedKind,
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
    Mixin(Box<MixinView>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageView {
    pub reference: String,
    pub digest: String,
}

/// A published mixin as its author wrote it: everything a sandbox may carry except the blocks that describe one launch, which a mixin may not declare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MixinView {
    pub reference: String,
    #[serde(default)]
    pub digest: String,
    /// The mixins this one declares, as written — a published mixin is shown unresolved, since what its graph merges to is a launch-time answer.
    #[serde(default)]
    pub mixins: Vec<String>,
    #[serde(default)]
    pub mounts: Vec<SandboxMount>,
    #[serde(default)]
    pub ports: Vec<SandboxPort>,
    #[serde(default)]
    pub filesets: Vec<SandboxFileset>,
    #[serde(default)]
    pub env: Vec<String>,
    /// A mixin may declare credentials of its own, and the merge unions them into the sandbox, so an inspect of the mixin alone has to show them.
    #[serde(default)]
    pub credentials: Vec<lns_spec::Credential>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub scripts: Vec<SandboxScript>,
    #[serde(default)]
    pub policy_flags: Vec<String>,
}

/// The block a contribution belongs to, which is what tells a renderer which line to attribute it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContributionBlock {
    Tool,
    Mount,
    Port,
    Egress,
    Script,
    Credential,
}

/// One `pre-start` script of a resolved sandbox, carried whole because a consumer approving a script has to be able to read it (`docs/sandbox-spec.md` §1.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxScript {
    pub when: String,
    pub run: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One entry of a resolved sandbox, named by the source that decided it and by whatever that decision replaced (`docs/sandbox-spec.md` §1.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceContribution {
    pub block: ContributionBlock,
    pub key: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub displaced: Vec<DisplacedEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplacedEntry {
    pub source: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxView {
    pub reference: String,
    #[serde(default)]
    pub digest: String,
    pub image: String,
    /// The mixins this sandbox resolved into, since the merged document declares none of its own.
    #[serde(default)]
    pub mixins: Vec<String>,
    /// What each mixin the user named resolved to, in the order they named them, so the run boots the bytes its preflight pinned.
    #[serde(default)]
    pub pinned_mixins: Vec<String>,
    /// Which source decided each entry below, and what that decision replaced.
    #[serde(default)]
    pub contributions: Vec<SourceContribution>,
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
    pub env: Vec<String>,
    /// The secrets the sandbox declares and the domains they may travel to: §1.5 approves a resolved sandbox, and a secret path nobody sees is one nobody approved.
    #[serde(default)]
    pub credentials: Vec<lns_spec::Credential>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub scripts: Vec<SandboxScript>,
    #[serde(default)]
    pub policy_flags: Vec<String>,
    /// What the sandbox declared, not what it resolves to — `None` must stay distinguishable from "declared exactly the default", or a flag can no longer outrank a declaration.
    #[serde(default)]
    pub cpus: Option<u8>,
    #[serde(default)]
    pub mem_mib: Option<usize>,
    #[serde(default)]
    pub disk_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxFileset {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub inline: bool,
    /// The host file the sandbox declared, verbatim — shape, never payload, so a consumer sees which of their files it reads.
    #[serde(default)]
    pub host_path: Option<String>,
    pub guest_path: String,
    #[serde(default)]
    pub owner: SandboxFilesetOwner,
    #[serde(default)]
    pub optional: bool,
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
    /// True when the published bind is skipped on a machine that lacks its source, rather than refusing the run.
    #[serde(default)]
    pub optional: bool,
    /// The capacity a published named volume asked for, so a pulled sandbox sizes its volume the way its author meant.
    #[serde(default)]
    pub size_bytes: Option<u64>,
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
    /// The run's own decisions file, filled in by the service from the run's directory; a caller never names it.
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
    pub detached: bool,
    #[serde(default)]
    pub auto_remove: bool,
}

impl RunConfig {
    pub fn from_run_args(args: &RunImageArgs) -> Self {
        Self {
            cpus: args.cpus,
            mem_mib: args.mem,
            policy_path: None,
            sandbox_user: args.sandbox_user.clone(),
            sandbox_uid: args.sandbox_uid,
            entrypoint: args.entrypoint.clone(),
            hostname: args.hostname.clone(),
            env: args.env.clone(),
            workdir: args.workdir.clone(),
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
    pub resolved_image: Option<String>,
    /// The mixins the preflight pinned, in the order the user named them; the run merges these, never a reference it has not resolved itself.
    #[serde(default)]
    pub mixins: Vec<String>,
    /// The same references, still named for a local run whose document the preflight already merged, so the composition the run ran is recorded whole.
    #[serde(default)]
    pub composed_mixins: Vec<String>,
    #[serde(default)]
    pub name: Option<String>,
    pub cpus: u8,
    pub mem: usize,
    /// True when the user set `--cpus`/`-m` explicitly, so a published sandbox's size can't silently override an explicit request that happens to equal the built-in default.
    #[serde(default)]
    pub cpus_explicit: bool,
    #[serde(default)]
    pub mem_explicit: bool,
    /// This machine's `run.cpus`/`run.mem` defaults, carried apart from the flag because they rank below the document: a sandbox that declares its own resources is not silently resized by whoever cloned it.
    #[serde(default)]
    pub cpus_config: Option<u8>,
    #[serde(default)]
    pub mem_config: Option<usize>,
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
    /// True when `image` is a reference the service must classify (refusing a plain OCI image that is not a sandbox); false for a local sandbox's base image, which the CLI has already resolved and the service runs directly.
    #[serde(default)]
    pub verify_sandbox: bool,
    /// A local sandbox definition as canonical JSON; the service plans it like a published sandbox so its policy and resources apply.
    #[serde(default)]
    pub definition: Option<String>,
    /// The local definition's absolute directory; absent for a published reference, which the service resolves by repo@digest instead.
    #[serde(default)]
    pub definition_dir: Option<String>,
    /// What the preflight resolved from every document source, as JSON egress: the gate folds the run's live decisions file over it, so a rule deleted mid-run retracts. Absent when the definition is the only source there was, and unread for a published reference (which the service resolves itself).
    #[serde(default)]
    pub authored_egress: Option<String>,
    /// Which artifact carries each of the merged definition's packed filesets, as the resolve answered; a published reference is peeked at boot and needs none of this.
    #[serde(default)]
    pub packed_filesets: Vec<PackedFilesetSource>,
    /// The `hostPath` sources this machine refused a pulled sandbox, so the guest is seeded from the files the developer granted and no others.
    #[serde(default)]
    pub denied_host_paths: Vec<String>,
}

/// One packed fileset's coordinates: the guest path it lands at, the digest-pinned artifact whose layer carries it, and that layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackedFilesetSource {
    pub guest_path: String,
    pub reference: String,
    pub digest: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeMount {
    pub name: String,
    pub target: String,
    pub read_only: bool,
    /// The capacity the volume must have before the run starts; the service grows a smaller one and leaves a larger one alone.
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

/// Splits a `:ro`/`:rw` suffix off a mount spec, so the two parsers cannot drift on which suffixes mean read-only.
fn split_read_only(spec: &str) -> (&str, bool) {
    let body = spec
        .strip_suffix(":ro")
        .or_else(|| spec.strip_suffix(":rw"))
        .unwrap_or(spec);
    (body, spec.ends_with(":ro"))
}

impl VolumeMount {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let (body, read_only) = split_read_only(spec);
        let (name, target) = body
            .split_once(':')
            .ok_or_else(|| format!("invalid volume {spec:?}: expected name:/path[:ro]"))?;
        validate_volume_name(name)?;
        validate_volume_target(target)?;
        Ok(Self {
            name: name.to_string(),
            target: target.to_string(),
            read_only,
            size_bytes: None,
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
    /// Subpaths the definition excluded, unioned with any `.lnsignore` before the secret scan.
    pub exclude: Vec<String>,
    /// True when an absent host source skips the bind instead of refusing the run.
    pub optional: bool,
}

impl BindSpec {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let (body, read_only) = split_read_only(spec);
        let (source, target) = body
            .split_once(':')
            .ok_or_else(|| format!("invalid host bind {spec:?}: expected /host-path:/path[:ro]"))?;
        validate_volume_target(target)?;
        validate_bind_source(source)?;
        // A `-v` carries neither exclude nor optional; the definition is the author surface for both.
        Ok(Self {
            host_source: source.to_string(),
            target: target.to_string(),
            read_only,
            exclude: Vec::new(),
            optional: false,
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
    #[serde(default)]
    pub tty: bool,
    #[serde(default)]
    pub stdin: bool,
    #[serde(default)]
    pub initial_winsize: Option<(u16, u16)>,
}

/// Which document `SaveRun` renders: the run as a whole, or only what it decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SaveKind {
    Sandbox,
    Mixin,
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
mod secret_tests {
    use super::*;

    #[test]
    fn a_debug_of_a_connect_request_cannot_print_the_value_it_carries() {
        // `Request` derives Debug and is formatted in panic and log paths, so a plaintext token must not be reachable through it.
        let request = Request::ConnectConnector {
            name: "some-provider".into(),
            method: "token".into(),
            connection: "work".into(),
            values: SecretValues([("SOME_TOKEN".to_string(), "sk-live-real".to_string())].into()),
        };
        let rendered = format!("{request:?}");
        assert!(!rendered.contains("sk-live-real"), "{rendered}");
        assert!(rendered.contains("redacted"), "{rendered}");
        assert!(
            rendered.contains("some-provider"),
            "everything that is not the value still has to be diagnosable: {rendered}"
        );
    }

    #[test]
    fn secret_values_still_travel_on_the_wire() {
        // Redaction is for Debug only; the service must receive what the user typed.
        let values = SecretValues([("SOME_TOKEN".to_string(), "sk-live-real".to_string())].into());
        let json = serde_json::to_string(&values).expect("serialises");
        assert_eq!(json, r#"{"SOME_TOKEN":"sk-live-real"}"#);
        let back: SecretValues = serde_json::from_str(&json).expect("round trips");
        assert_eq!(back, values);
    }
}

#[cfg(test)]
mod tests {

    fn a_method_setting(env: &[&str], credentials: &[&str]) -> ConnectorMethodView {
        ConnectorMethodView {
            name: "token".into(),
            label: "token".into(),
            auth_label: Some("token".into()),
            offerable: true,
            opens: Vec::new(),
            writes: Vec::new(),
            env: env.iter().map(|e| (*e).to_string()).collect(),
            credentials: credentials.iter().map(|c| (*c).to_string()).collect(),
            asks: Vec::new(),
            help: None,
            overrides: None,
        }
    }

    #[test]
    fn the_variables_a_method_sets_are_its_env_and_the_one_each_credential_fills() {
        // §3.2.4: both the card and `lns connector grant` disclose this, and two spellings of it would let one of them omit a variable the other names.
        assert_eq!(
            a_method_setting(&["SOME_REGION"], &["SOME_TOKEN"]).sets(),
            ["SOME_REGION", "SOME_TOKEN"]
        );
        assert!(a_method_setting(&[], &[]).sets().is_empty());
    }

    fn holding(labels: &[&str]) -> ConnectorView {
        ConnectorView {
            name: "some-provider".into(),
            digest: "sha256:abc".into(),
            serves: vec!["api.some-provider.example".into()],
            methods: Vec::new(),
            connections: labels
                .iter()
                .map(|label| ConnectorConnectionView {
                    label: label.to_string(),
                    method: "token".into(),
                    authority: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn a_new_connection_is_named_after_its_method_when_nothing_holds_that_name() {
        assert_eq!(holding(&[]).free_connection_name("token"), "token");
    }

    #[test]
    fn a_name_already_held_is_stepped_past_rather_than_replaced() {
        // Replacing is what a colliding label does in the store, silently, taking every grant that named it.
        assert_eq!(holding(&["token"]).free_connection_name("token"), "token-2");
        assert_eq!(
            holding(&["token", "token-2"]).free_connection_name("token"),
            "token-3"
        );
    }

    #[test]
    fn a_gap_left_by_a_disconnect_is_filled_rather_than_counted_past() {
        // Counting the accounts would suggest `token-2` here and replace the one account this machine still holds.
        assert_eq!(holding(&["token-2"]).free_connection_name("token"), "token");
    }

    #[test]
    fn a_label_another_method_holds_still_collides() {
        // The store's key is the connector and the label; the method is recorded in the value, so reusing the name rewrites that too.
        let mut view = holding(&["token"]);
        view.connections[0].method = "session".into();
        assert_eq!(view.free_connection_name("token"), "token-2");
    }
    use super::*;

    #[test]
    fn a_script_survives_the_wire_with_its_body_slot_user_and_description() {
        let step = SandboxScript {
            when: "pre-start".to_string(),
            run: "apt-get install -y psql".to_string(),
            user: Some("root".to_string()),
            description: Some("the psql the prompts assume".to_string()),
        };
        let round_tripped: SandboxScript =
            serde_json::from_str(&serde_json::to_string(&step).expect("a script encodes"))
                .expect("a script decodes");
        assert_eq!(
            round_tripped, step,
            "the disclosure shows the body, the user it asks for, and what it says about itself, so none of the three may be lost on the way to the CLI"
        );
    }

    #[test]
    fn a_view_that_predates_scripts_decodes_as_declaring_none() {
        let view: SandboxView = serde_json::from_value(serde_json::json!({
            "reference": "ghcr.io/acme/reviewer",
            "image": "ghcr.io/acme/base@sha256:abc",
        }))
        .expect("every added block defaults");
        assert!(view.scripts.is_empty());
        let mixin: MixinView = serde_json::from_value(serde_json::json!({
            "reference": "ghcr.io/acme/postgres-tools",
        }))
        .expect("every added block defaults");
        assert!(mixin.scripts.is_empty());
    }

    #[test]
    fn the_script_contribution_block_names_itself_on_the_wire() {
        assert_eq!(
            serde_json::to_string(&ContributionBlock::Script).expect("a block encodes"),
            "\"script\"",
            "a renderer picks the line to attribute by this name, so it is part of the contract rather than an internal spelling"
        );
        assert_eq!(
            serde_json::to_string(&ContributionBlock::Credential).expect("a block encodes"),
            "\"credential\"",
            "§1.5 attributes every credential of a resolved sandbox, so the block a renderer matches on is part of the contract too"
        );
    }

    #[test]
    fn exec_image_args_terminal_flags_default_to_false_when_missing() {
        let frame = serde_json::json!({
            "run": "1",
            "argv": ["sh"],
            "env": []
        });
        let parsed: ExecImageArgs = serde_json::from_value(frame).unwrap();
        assert!(!parsed.tty);
        assert!(!parsed.stdin);
    }

    #[test]
    fn session_control_requests_round_trip_with_explicit_targets() {
        for frame in [
            serde_json::json!({
                "type": "SessionStdin",
                "target": {
                    "kind": "exec",
                    "run_id": "7",
                    "session_id": "exec-1"
                },
                "bytes": [104, 105]
            }),
            serde_json::json!({
                "type": "SessionResize",
                "target": {
                    "kind": "primary",
                    "run_id": "7"
                },
                "rows": 24,
                "cols": 80
            }),
            serde_json::json!({
                "type": "SessionSignal",
                "target": {
                    "kind": "exec",
                    "run_id": "7",
                    "session_id": "exec-1"
                },
                "signal": "int"
            }),
            serde_json::json!({
                "type": "SessionDetach",
                "target": {
                    "kind": "exec",
                    "run_id": "7",
                    "session_id": "exec-1"
                }
            }),
            serde_json::json!({
                "type": "SessionStdinClose",
                "target": {
                    "kind": "exec",
                    "run_id": "7",
                    "session_id": "exec-1"
                }
            }),
        ] {
            let parsed: Request = serde_json::from_value(frame.clone())
                .expect("session control request should decode");
            assert_eq!(serde_json::to_value(parsed).unwrap(), frame);
        }
    }

    #[test]
    fn exec_started_response_round_trips_with_session_id() {
        let frame = serde_json::json!({
            "type": "ExecStarted",
            "run_id": "7",
            "session_id": "exec-1"
        });
        let parsed: Response =
            serde_json::from_value(frame.clone()).expect("ExecStarted response should decode");
        assert_eq!(serde_json::to_value(parsed).unwrap(), frame);
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
            resolved_image: None,
            mixins: Vec::new(),
            composed_mixins: Vec::new(),
            name: None,
            cpus: 1,
            mem: 512,
            cpus_explicit: false,
            mem_explicit: false,
            cpus_config: None,
            mem_config: None,
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
            verify_sandbox: false,
            definition: None,
            definition_dir: None,
            authored_egress: None,
            packed_filesets: Vec::new(),
            denied_host_paths: Vec::new(),
        }));
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn run_image_args_declarative_launch_settings_survive_postcard_round_trip() {
        let args = RunImageArgs {
            image: Some("ubuntu".into()),
            resolved_image: Some(format!("ubuntu@sha256:{}", "a".repeat(64))),
            mixins: Vec::new(),
            composed_mixins: Vec::new(),
            name: None,
            cpus: 1,
            mem: 512,
            cpus_explicit: false,
            mem_explicit: false,
            cpus_config: None,
            mem_config: None,
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
                size_bytes: None,
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
            authored_egress: None,
            packed_filesets: vec![PackedFilesetSource {
                guest_path: "/root/.agent/skills".into(),
                reference: format!("ghcr.io/acme/skills@sha256:{}", "b".repeat(64)),
                digest: format!("sha256:{}", "c".repeat(64)),
                size: 4096,
            }],
            denied_host_paths: Vec::new(),
        };
        let frame = crate::encode_frame(&args).unwrap();
        let decoded: RunImageArgs = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(
            decoded, args,
            "a mixin's packed fileset is a layer of the mixin's own artifact, so the run has to carry which artifact and which layer or the boot cannot materialize it"
        );
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
            resolved_image: None,
            mixins: Vec::new(),
            composed_mixins: Vec::new(),
            name: None,
            cpus: 2,
            mem: 1024,
            cpus_explicit: true,
            mem_explicit: true,
            cpus_config: None,
            mem_config: None,
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
                size_bytes: None,
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
            definition: Some(r#"{"kind":"sandbox"}"#.into()),
            definition_dir: Some("/work/proj".into()),
            authored_egress: Some(r#"{"http":[],"tcp":[]}"#.into()),
            packed_filesets: Vec::new(),
            denied_host_paths: Vec::new(),
        }
    }

    #[test]
    fn run_config_from_run_args_carries_the_launch_configuration() {
        let args = sample_run_args();
        let config = RunConfig::from_run_args(&args);
        assert_eq!(config.cpus, 2);
        assert_eq!(config.mem_mib, 1024);
        assert_eq!(
            config.policy_path, None,
            "a run's decisions file is the service's to name from the run's own directory, so a caller's launch args must not carry one"
        );
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
        let req = Request::RemoveRun {
            run: "7".into(),
            force: true,
        };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn start_run_survives_a_request_round_trip() {
        let req = Request::StartRun {
            run: "reviewer".into(),
            attach: true,
            stdin: true,
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
    fn a_config_default_travels_in_its_own_slot_apart_from_the_flags() {
        let mut args = sample_run_args();
        args.cpus_explicit = false;
        args.mem_explicit = false;
        args.cpus_config = Some(4);
        args.mem_config = Some(4096);
        let frame = crate::encode_frame(&args).unwrap();
        let decoded: RunImageArgs = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(
            (
                decoded.cpus_explicit,
                decoded.mem_explicit,
                decoded.cpus_config,
                decoded.mem_config
            ),
            (false, false, Some(4), Some(4096)),
            "a configured default must not reach the service as an explicit flag, or it would outrank the document"
        );
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
    fn primary_session_detach_request_survives_a_round_trip() {
        let req = Request::SessionDetach {
            target: SessionTarget::Primary {
                run_id: "7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a".to_string(),
            },
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
            Request::ListPrunableImages,
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
            kind: CachedKind::Sandbox,
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
    fn an_inspect_request_carries_the_mixins_the_user_named_and_defaults_to_none() {
        let req = Request::InspectImage {
            image: "registry.example.test/team/sandbox:1".into(),
            mixins: vec!["ghcr.io/acme/obs-tools:2".into()],
        };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
        let bare: Request =
            serde_json::from_str(r#"{"type":"InspectImage","image":"registry.example.test/x:1"}"#)
                .expect("an inspect naming no mixin decodes");
        assert!(
            matches!(bare, Request::InspectImage { ref mixins, .. } if mixins.is_empty()),
            "an inspect with no flag asks about the artifact alone, so it must not invent a source the user never named"
        );
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
    fn registry_login_stored_response_survives_a_round_trip() {
        let resp = Response::RegistryLoginStored;
        let frame = crate::encode_frame(&resp).unwrap();
        let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, resp);
    }

    #[test]
    fn registry_logout_request_survives_a_round_trip() {
        let req = Request::RegistryLogout {
            registry: "registry.example.test".into(),
        };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn registry_logged_out_response_survives_a_round_trip() {
        let resp = Response::RegistryLoggedOut;
        let frame = crate::encode_frame(&resp).unwrap();
        let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, resp);
    }

    #[test]
    fn registry_login_list_round_trips_hosts_and_usernames_but_has_no_room_for_secrets() {
        let req = Request::ListRegistryLogins;
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);

        let resp = Response::RegistryLogins {
            logins: vec![RegistryLoginSummary {
                registry: "registry.example.test".into(),
                username: "someone".into(),
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(
            !json.contains("secret"),
            "the list surface carries hosts and usernames only; got {json}"
        );
        let decoded: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, resp);
    }

    #[test]
    fn a_cached_mixin_row_names_its_kind_in_words_and_on_the_wire() {
        assert_eq!(CachedKind::Mixin.as_str(), "mixin");
        let json = serde_json::to_value(CachedKind::Mixin).unwrap();
        assert_eq!(json, serde_json::json!("mixin"));
        let decoded: CachedKind = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, CachedKind::Mixin);
    }

    #[test]
    fn image_info_serializes_idle_holder_as_null() {
        let info = ImageInfo {
            reference: "registry.example.test/some/image:1.0".into(),
            kind: CachedKind::Sandbox,
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
    fn sandbox_view_round_trips_declarative_launch_settings() {
        let view = SandboxView {
            mixins: vec!["ghcr.io/acme/postgres-tools@sha256:c41e8b7d20a95f6c3d84b1e07f92a5c8d63b40e19a7c25f8b0d3e6a94c17f582".into()],
            pinned_mixins: Vec::new(),
            contributions: Vec::new(),
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
                optional: false,
                size_bytes: None,
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
                path: Some("./skills".into()),
                inline: false,
                host_path: None,
                optional: false,
                guest_path: "/root/.agent/skills".into(),
                owner: SandboxFilesetOwner::Workload,
            }],
            credentials: Vec::new(),
            env: vec!["SHELL=/bin/sh".into()],
            tools: vec!["node@22.11.0".into()],
            scripts: vec![SandboxScript {
                when: "pre-start".into(),
                run: "apt-get install -y --no-install-recommends jq".into(),
                user: Some("root".into()),
                description: Some("the jq the prompts assume".into()),
            }],
            policy_flags: Vec::new(),
            cpus: Some(3),
            mem_mib: Some(6144),
            disk_bytes: Some(40 << 30),
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
                size_bytes: None,
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
                optional: false,
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
                size_bytes: None,
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
