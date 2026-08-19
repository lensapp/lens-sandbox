use anyhow::{Context, Result, anyhow, bail};
use lns_policy::Egress;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::spec::{self, Port, Resources};

pub const API_VERSION: &str = "lns.run/v1";
pub const MAX_INLINE_FILE_BYTES: usize = 128 * 1024;
pub const MAX_INLINE_TOTAL_BYTES: usize = 1024 * 1024;
pub const MAX_INLINE_FILES: usize = 256;
pub const MAX_INLINE_PATH_BYTES: usize = 4096;

const EXACT_SECRET_NAMES: &[&str] = &[
    ".npmrc",
    ".netrc",
    ".git-credentials",
    ".pgpass",
    ".pypirc",
    ".yarnrc.yml",
    "auth.json",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "credentials",
    ".ssh",
    ".aws",
    ".gnupg",
    ".kube",
    ".azure",
    ".oci",
    ".docker",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub name: String,
    pub spec: SandboxSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VolumeType {
    Bind,
    Volume,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Volume {
    #[serde(rename = "type", default)]
    volume_type: Option<VolumeType>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    name: Option<String>,
    pub target: String,
    #[serde(default)]
    read_only: bool,
    /// Subpaths of a bind the workload must not see, relative to the bind root; masked in the guest exactly as a `.lensignore` entry is.
    #[serde(default)]
    exclude: Vec<String>,
    /// A bind source the running machine does not have is skipped instead of refusing the run.
    #[serde(default)]
    optional: bool,
    /// The capacity a named volume must have before the run starts; the service grows a smaller one and never shrinks a larger one.
    #[serde(default)]
    pub size: Option<spec::Quantity>,
}

impl Volume {
    pub fn exclude(&self) -> &[String] {
        &self.exclude
    }

    pub fn optional(&self) -> bool {
        self.optional
    }

    pub fn source(&self) -> &str {
        self.source
            .as_deref()
            .or(self.name.as_deref())
            .unwrap_or_default()
    }

    pub fn is_bind(&self) -> bool {
        self.volume_type == Some(VolumeType::Bind)
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// The capacity this volume must have, in bytes; `None` when the document named none, because validation already refused any size that does not parse.
    pub fn size_bytes(&self) -> Option<u64> {
        self.size
            .as_ref()
            .and_then(|q| crate::disk::parse_bytes(q).ok())
    }
}

/// The whole sandbox in one document: the base image plus its config, env, embedded network policy, mounts, and the connector ids it needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SandboxSpec {
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub workdir: Option<String>,
    /// The run-as user the sandbox needs, `USER[:GROUP]` like `-u`; the flag still wins, and the image's own `USER` is the fallback.
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub resources: Option<Resources>,
    #[serde(default)]
    pub egress: Egress,
    #[serde(default)]
    pub connectors: Vec<String>,
    #[serde(default)]
    pub credentials: Vec<lns_spec::Credential>,
    #[serde(default)]
    pub mixins: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub volumes: Vec<Volume>,
    #[serde(default)]
    pub filesets: Vec<FilesetEntry>,
    #[serde(default)]
    pub ports: Vec<Port>,
}

/// Files shipped inside the artifact: a directory beside this document packed into a layer of the same artifact at push (path), or content written in the document itself (inline), snapshot-mounted at guestPath. A hostPath instead names one file on the machine that runs it, snapshotted at launch and never packed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilesetEntry {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub inline: Option<BTreeMap<String, String>>,
    #[serde(rename = "hostPath", default)]
    pub host_path: Option<String>,
    #[serde(rename = "guestPath")]
    pub guest_path: String,
    #[serde(default)]
    pub owner: FilesetOwner,
    /// A hostPath the running machine does not have is skipped instead of refusing the run.
    #[serde(default)]
    pub optional: bool,
}

/// Who owns the materialized files in the guest: the run-as workload user (so the workload can rewrite its own seeded state), or root (pinned inputs the workload must not touch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FilesetOwner {
    #[default]
    Workload,
    Root,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Doc {
    #[serde(rename = "apiVersion", default)]
    api_version: String,
    #[serde(default)]
    kind: String,
    name: String,
    /// Read only after the group and kind decide, so another group's field names cannot answer as unknown fields of this one.
    #[serde(default)]
    spec: serde_json::Value,
}

/// Parse and cross-field-validate a `lns.run/v1` sandbox definition, offline.
pub fn parse(config_json: &[u8]) -> Result<Definition> {
    let doc = parse_of_kind(config_json, spec::Kind::Sandbox)?;
    if doc.spec.image.trim().is_empty() {
        bail!("sandbox must carry an image; it is the base OCI image the sandbox runs");
    }
    Ok(doc)
}

/// Parse and cross-field-validate a `lns.run/v1` mixin, offline. Its blocks follow the same rules as a sandbox's; what differs is that the five describing one launch are forbidden, and there is no image to require.
pub fn parse_mixin(config_json: &[u8]) -> Result<Definition> {
    parse_of_kind(config_json, spec::Kind::Mixin)
}

/// Whether a mixin entry names a directory rather than a registry coordinate — the one predicate validation, rooting and resolution all read, so they cannot disagree about which entries are local.
pub fn names_a_local_path(reference: &str) -> bool {
    reference == "."
        || reference == ".."
        || reference.starts_with("./")
        || reference.starts_with("../")
        || reference.starts_with('/')
}

/// Fold `..` away without touching the filesystem, so one directory named two ways is one path; `components` has already dropped every `.` these callers can pass, since each joins onto an absolute base.
pub fn fold_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut folded = std::path::PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                folded.pop();
            }
            other => folded.push(other),
        }
    }
    folded
}

/// A remote mixin reference must be digest-pinned, because a published document has to resolve to the same thing for everyone.
fn validate_mixin_reference(reference: &str) -> Result<()> {
    if reference.trim().is_empty() {
        bail!("a mixin entry must name a directory or an OCI reference");
    }
    if !names_a_local_path(reference) && !spec::is_digest_pinned_image(reference) {
        bail!(
            "mixin reference {reference:?} must be digest-pinned (…@sha256:<64 hex>), so every consumer resolves the same document; a local path starts with `./`, `../` or `/`"
        );
    }
    Ok(())
}

/// The blocks a mixin may not carry: the five that describe one launch, which the sandbox owns, plus the connector list — how a credential is obtained is decided per machine, never by a document that travels with a workload.
fn refuse_blocks_a_mixin_cannot_carry(spec: &SandboxSpec) -> Result<()> {
    let launch_blocks = [
        (!spec.image.trim().is_empty(), "image"),
        (spec.command.is_some(), "command"),
        (spec.workdir.is_some(), "workdir"),
        (spec.user.is_some(), "user"),
        (spec.resources.is_some(), "resources"),
    ];
    if let Some((_, block)) = launch_blocks.iter().find(|(declared, _)| *declared) {
        bail!("a mixin must not declare {block}: it describes one launch, and the sandbox owns it");
    }
    if !spec.connectors.is_empty() {
        bail!(
            "a mixin must not name a connector: which method supplies a credential is decided per machine"
        );
    }
    Ok(())
}

fn parse_of_kind(config_json: &[u8], kind: spec::Kind) -> Result<Definition> {
    let doc: Doc = serde_json::from_slice(config_json).context("parsing sandbox definition")?;
    if doc.api_version != API_VERSION {
        bail!(
            "unexpected apiVersion {:?}; expected {API_VERSION}",
            doc.api_version
        );
    }
    if doc.kind != kind.as_str() {
        bail!(
            "expected kind {} but definition declares {:?}",
            kind.as_str(),
            doc.kind
        );
    }
    if !spec::is_valid_name(&doc.name) {
        bail!("invalid name {:?}", doc.name);
    }
    let doc = Definition {
        name: doc.name,
        spec: serde_json::from_value(doc.spec).context("parsing sandbox spec")?,
    };
    if kind == spec::Kind::Mixin {
        refuse_blocks_a_mixin_cannot_carry(&doc.spec)?;
    }
    for key in doc.spec.env.keys() {
        if !lns_spec::is_legal_env_var_name(key) {
            bail!(
                "invalid env key {key:?}: env keys must be non-empty and free of '=', whitespace, and control characters"
            );
        }
    }
    if let Some(user) = &doc.spec.user {
        validate_run_as_user(user)?;
    }
    if let Some(resources) = &doc.spec.resources {
        validate_resources(resources)?;
    }
    doc.spec
        .egress
        .validate_local_transport()
        .context("sandbox policy")?;
    doc.spec
        .egress
        .validate_binary_scopes()
        .context("sandbox policy")?;
    if let Some(workdir) = &doc.spec.workdir {
        spec::validate_guest_path(workdir).context("workdir")?;
    }
    let mut targets = BTreeSet::new();
    for volume in &doc.spec.volumes {
        spec::validate_guest_path(&volume.target)
            .with_context(|| format!("volume targeting {}", volume.target))?;
        if overlaps_runtime_namespace(&volume.target) {
            bail!(
                "volume target {} overlaps the /.lens runtime namespace, which belongs to the sandbox itself",
                volume.target
            );
        }
        validate_volume(volume).with_context(|| format!("volume targeting {}", volume.target))?;
        if !targets.insert(&volume.target) {
            bail!("duplicate volume target {}", volume.target);
        }
    }
    for connector in &doc.spec.connectors {
        if !spec::is_valid_name(connector) {
            bail!("invalid connector id {connector:?}");
        }
    }
    lns_spec::credential::validate_all(&doc.spec.credentials)
        .map_err(|problem| anyhow!(problem))?;
    for reference in &doc.spec.mixins {
        validate_mixin_reference(reference)?;
    }
    crate::tools::parse_all(&doc.spec.tools)?;
    for fileset in &doc.spec.filesets {
        validate_fileset(fileset)?;
        if !targets.insert(&fileset.guest_path) {
            bail!("duplicate guest path {}", fileset.guest_path);
        }
    }
    let mut container_ports = BTreeSet::new();
    let mut host_ports = BTreeSet::new();
    for port in &doc.spec.ports {
        if !(1..=65535).contains(&port.container) {
            bail!(
                "sandbox port container {} is out of range (1-65535)",
                port.container
            );
        }
        if !container_ports.insert(port.container) {
            bail!("duplicate container port {}", port.container);
        }
        if let Some(host) = port.host {
            if !(1..=65535).contains(&host) {
                bail!("sandbox port host {host} is out of range (1-65535)");
            }
            if !host_ports.insert(host) {
                bail!("duplicate host port {host}");
            }
        }
    }
    Ok(doc)
}

fn validate_fileset(fileset: &FilesetEntry) -> Result<()> {
    let source_count = usize::from(fileset.path.is_some())
        + usize::from(fileset.inline.is_some())
        + usize::from(fileset.host_path.is_some());
    if source_count != 1 || fileset.inline.as_ref().is_some_and(BTreeMap::is_empty) {
        bail!(
            "fileset targeting {} must set exactly one of path, inline, or hostPath",
            fileset.guest_path
        );
    }
    if let Some(path) = &fileset.path {
        if path.is_empty() {
            bail!("fileset path must not be empty");
        }
        if path.starts_with('~') {
            bail!(
                "fileset path {path:?} is packed from a directory beside this document, so it cannot be home-anchored; use hostPath to read one file from the machine that runs the sandbox"
            );
        }
    }
    if let Some(inline) = &fileset.inline {
        validate_inline_files(inline)?;
    }
    match &fileset.host_path {
        Some(host_path) => {
            validate_host_source(host_path)?;
            if fileset.guest_path.ends_with('/') {
                bail!(
                    "fileset hostPath {host_path} names a guest file, so guestPath {} must not end in `/`",
                    fileset.guest_path
                );
            }
        }
        None if fileset.optional => bail!(
            "optional applies to a hostPath fileset only; a packed or inline fileset always ships"
        ),
        None => {}
    }
    spec::validate_guest_path(&fileset.guest_path).context("fileset guestPath")?;
    if overlaps_runtime_namespace(&fileset.guest_path) {
        bail!(
            "fileset guestPath {} overlaps the /.lens runtime namespace, which belongs to the sandbox itself",
            fileset.guest_path
        );
    }
    Ok(())
}

fn validate_inline_files(inline: &BTreeMap<String, String>) -> Result<()> {
    if inline.len() > MAX_INLINE_FILES {
        bail!(
            "inline fileset has {} files, more than the {MAX_INLINE_FILES}-file limit; use a path fileset",
            inline.len()
        );
    }
    let mut total_bytes: usize = 0;
    for (path, content) in inline {
        validate_inline_path(path)?;
        if content.len() > MAX_INLINE_FILE_BYTES {
            bail!(
                "inline file {path:?} exceeds the {MAX_INLINE_FILE_BYTES}-byte limit; use a path fileset"
            );
        }
        total_bytes += content.len();
    }
    if total_bytes > MAX_INLINE_TOTAL_BYTES {
        bail!(
            "inline fileset totals {total_bytes} bytes, more than the {MAX_INLINE_TOTAL_BYTES}-byte limit; use a path fileset"
        );
    }
    Ok(())
}

fn validate_inline_path(path: &str) -> Result<()> {
    if path.len() > MAX_INLINE_PATH_BYTES {
        bail!(
            "inline file path exceeds the {MAX_INLINE_PATH_BYTES}-byte limit; use a shorter path"
        );
    }
    let segments: Vec<&str> = path.split('/').collect();
    if path.is_empty()
        || path.starts_with('/')
        || segments
            .iter()
            .any(|segment| segment.is_empty() || *segment == "." || *segment == "..")
        || path.chars().any(char::is_control)
    {
        bail!(
            "inline file path {path:?} must be a safe relative path beneath its fileset guestPath"
        );
    }
    if segments
        .iter()
        .any(|segment| looks_like_secret_name(segment))
    {
        bail!(
            "inline fileset contains a secret-shaped file: {path} — real secrets stay outside the workload"
        );
    }
    Ok(())
}

pub fn looks_like_secret_name(name: &str) -> bool {
    name.starts_with(".env")
        || name.starts_with("credentials.")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.ends_with(".ppk")
        || name.ends_with(".keystore")
        || EXACT_SECRET_NAMES.contains(&name)
}

fn overlaps_runtime_namespace(path: &str) -> bool {
    match path
        .split('/')
        .find(|segment| !segment.is_empty() && *segment != ".")
    {
        None => true,
        Some(first) => first == ".lens",
    }
}

/// The value reaches the guest on a space-joined kernel cmdline, so anything that could split it or be read as another key is refused here — the one offline check every load path shares.
fn validate_run_as_user(user: &str) -> Result<()> {
    let mut segments = user.split(':');
    let name = segments.next().unwrap_or_default();
    let group = segments.next();
    if segments.next().is_some() {
        bail!("invalid user {user:?}: expected USER or USER:GROUP");
    }
    for segment in [Some(name), group].into_iter().flatten() {
        // A quote is as dangerous as whitespace here: both the kernel's parse_args and lns-init's own tokenizer honour it, so one would swallow every key after this into its value.
        if segment.is_empty()
            || !lns_spec::is_legal_env_var_name(segment)
            || segment.contains('"')
            || segment.contains('\'')
        {
            bail!(
                "invalid user {user:?}: each of USER and GROUP must be non-empty and free of '=', quotes, whitespace, and control characters"
            );
        }
    }
    Ok(())
}

/// A cpu request is a positive count — a bare integer ≥ 1 or a millicore string like `500m` — while the service's resolver keeps ownership of the host ceiling and the fallback for anything else.
fn cpu_is_positive(quantity: &spec::Quantity) -> bool {
    match quantity {
        spec::Quantity::Int(n) => *n >= 1,
        spec::Quantity::Text(text) if crate::resources::parse_percent(text).is_some() => true,
        spec::Quantity::Text(text) => {
            let digits = text
                .trim()
                .trim_end_matches(|c: char| c.is_ascii_alphabetic());
            !digits.is_empty()
                && digits.bytes().all(|b| b.is_ascii_digit())
                && digits.bytes().any(|b| b != b'0')
        }
    }
}

fn validate_resources(resources: &Resources) -> Result<()> {
    if let Some(cpu) = &resources.cpu
        && !cpu_is_positive(cpu)
    {
        bail!("resources.cpu {cpu:?} must be a positive count or size");
    }
    match &resources.memory {
        Some(memory @ spec::Quantity::Int(n)) if *n < 1 => {
            bail!("resources.memory {memory:?} must be a positive count or size")
        }
        // A share resolves against the host at launch, so there is nothing to read here beyond its shape.
        Some(spec::Quantity::Text(text)) if crate::resources::parse_percent(text).is_some() => {}
        Some(spec::Quantity::Text(text)) => {
            crate::memory::parse_mib(text).context("resources.memory")?;
        }
        _ => {}
    }
    if let Some(disk) = &resources.disk {
        crate::disk::parse_bytes(disk).context("resources.disk")?;
    }
    Ok(())
}

fn validate_volume(volume: &Volume) -> Result<()> {
    if !volume.exclude.is_empty() && !volume.is_bind() {
        bail!("exclude applies to a bind volume only; a named volume has no host subpaths to hide");
    }
    if volume.optional && !volume.is_bind() {
        bail!(
            "optional applies to a bind volume only; a named volume is created on demand, never absent"
        );
    }
    if let Some(size) = &volume.size {
        if volume.is_bind() {
            bail!(
                "size applies to a named volume only; a bind is whatever the host path already holds"
            );
        }
        crate::disk::parse_bytes(size).context("volumes[].size")?;
    }
    for entry in &volume.exclude {
        validate_bind_relative_path(entry)?;
    }
    match volume.volume_type {
        Some(VolumeType::Bind) => {
            if volume.name.is_some() {
                bail!("bind volume must use source, not name");
            }
            validate_bind_source(volume.source.as_deref().unwrap_or_default())
        }
        Some(VolumeType::Volume) => {
            if volume.source.is_some() && volume.name.is_some() {
                bail!("named volume must use either source or name, not both");
            }
            validate_volume_name(volume.source())
        }
        None => {
            if volume.source.is_some() {
                bail!("volume with source must declare type: bind or type: volume");
            }
            validate_volume_name(volume.name.as_deref().unwrap_or_default())
        }
    }
}

/// An exclude names a path inside the bind, and it reaches the guest as a kernel-cmdline token, so anything that could escape the bind or split the line is refused here.
fn validate_bind_relative_path(entry: &str) -> Result<()> {
    if entry.is_empty() {
        bail!("exclude entry must not be empty");
    }
    if entry.starts_with('/') {
        bail!("invalid exclude {entry:?}: must be relative to the bind, not absolute");
    }
    // An empty segment covers a trailing slash and `a//b`: the gitignore habit `\".cargo/\"` would stat with the slash, miss, and leave the path the author excluded fully exposed.
    if entry
        .split('/')
        .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        bail!("invalid exclude {entry:?}: must not contain empty, `.`, or `..` path segments");
    }
    if entry
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\'')
    {
        bail!(
            "invalid exclude {entry:?}: must be free of whitespace, quotes, and control characters"
        );
    }
    Ok(())
}

/// A `~/` source is the one form a published definition can aim at the consumer's own home, and the keep/drop scan it meets later reads top-level names only, so a secret-shaped segment is refused here; an absolute or project-relative source keeps the rules it already had.
fn validate_bind_source(source: &str) -> Result<()> {
    if source.is_empty() {
        bail!("bind source must not be empty");
    }
    if source.starts_with('~') && !source.starts_with("~/") {
        bail!("invalid bind source {source:?}: only `~/` is supported, not another user's home");
    }
    if source
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '"')
    {
        bail!("bind source {source:?} must not contain whitespace, quotes, or control characters");
    }
    if source.split('/').any(|segment| segment == "..") {
        bail!("bind source {source:?} must not contain a `..` path segment");
    }
    if source.starts_with("~/") {
        refuse_secret_shaped_segment("bind source", source)?;
    }
    Ok(())
}

fn refuse_secret_shaped_segment(kind: &str, source: &str) -> Result<()> {
    if let Some(secret) = source.split('/').find(|s| looks_like_secret_name(s)) {
        bail!(
            "{kind} {source} is secret-shaped ({secret}) — real secrets stay outside the workload"
        );
    }
    Ok(())
}

/// A hostPath names one file on the machine that runs the definition; it must anchor somewhere portable, stay inside what it anchors to, and never be secret-shaped — a hostPath file gets no KEEP/DROP prompt, so its name is the only guard it has.
pub fn validate_host_source(source: &str) -> Result<()> {
    if source.starts_with('~') && !source.starts_with("~/") {
        bail!("invalid hostPath {source:?}: only `~/` is supported, not another user's home");
    }
    if !(source.starts_with('/') || source.starts_with("~/")) {
        bail!("invalid hostPath {source:?}: must start with `/` or `~/`");
    }
    if source.split('/').any(|segment| segment == "..") {
        bail!("invalid hostPath {source:?}: must not contain a `..` path segment");
    }
    if source
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\'')
    {
        bail!(
            "invalid hostPath {source:?}: must be free of whitespace, quotes, and control characters"
        );
    }
    refuse_secret_shaped_segment("hostPath", source)
}

fn validate_volume_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("named volume source must not be empty");
    }
    if name == "." || name == ".." {
        bail!("invalid named volume source {name:?}: reserved");
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
    {
        bail!("invalid named volume source {name:?}: character {bad:?} not allowed");
    }
    Ok(())
}

/// Parse whichever `lns.run/v1` document this is, so every verb answers for the kind the file declares rather than the one it expected.
pub fn parse_document(config_json: &[u8]) -> Result<Definition> {
    if spec::read_kind(config_json).ok() == Some(spec::Kind::Mixin) {
        return parse_mixin(config_json);
    }
    parse(config_json)
}

/// Schema + cross-field guards for whichever `lns.run/v1` document this is.
pub fn validate(config_json: &[u8]) -> Result<()> {
    parse_document(config_json).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_a_path_leaves_one_spelling_for_one_directory() {
        assert_eq!(
            fold_path(std::path::Path::new("/work/./mixins/../mixins/pg")),
            std::path::PathBuf::from("/work/mixins/pg"),
            "two spellings that reach one directory have to fold to one, or a walk would read it twice under two identities"
        );
    }

    fn def_json(spec: &str) -> Vec<u8> {
        format!(r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{spec}}}"#)
            .into_bytes()
    }

    fn mixin_json(spec: &str) -> Vec<u8> {
        format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"mixin","name":"postgres-tools","spec":{spec}}}"#
        )
        .into_bytes()
    }

    const PINNED: &str = "ghcr.io/acme/postgres-tools@sha256:c41e8b7d20a95f6c3d84b1e07f92a5c8d63b40e19a7c25f8b0d3e6a94c17f582";

    #[test]
    fn a_mixin_reads_every_block_it_shares_with_a_sandbox() {
        let def = parse_mixin(&mixin_json(
            r#"{"env":{"MODE":"research"},"tools":["postgresql@17"],"egress":{"tcp":[{"match":"db.example.com:5432","verdict":"allow"}]},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"lns-placeholder-some-token"}],"filesets":[{"inline":{"USING-POSTGRES.md":"Connect with $DATABASE_URL."},"guestPath":"/home/agent/notes"}],"ports":[{"container":8080}],"volumes":[{"type":"volume","name":"cache","target":"/home/agent/.cache"}]}"#,
        ))
        .expect("a mixin's shared blocks follow the same rules as a sandbox's");
        assert_eq!(def.name, "postgres-tools");
        assert_eq!(def.spec.tools, vec!["postgresql@17".to_string()]);
        assert_eq!(
            def.spec.env.get("MODE").map(String::as_str),
            Some("research")
        );
        assert_eq!(def.spec.ports[0].container, 8080);
        assert_eq!(def.spec.volumes[0].target, "/home/agent/.cache");
        assert_eq!(def.spec.egress.tcp.len(), 1);
        assert_eq!(def.spec.credentials[0].env_var, "SOME_TOKEN");
        assert_eq!(def.spec.filesets[0].guest_path, "/home/agent/notes");
    }

    #[test]
    fn a_mixin_refuses_each_block_that_describes_one_launch() {
        for (spec, block) in [
            (r#"{"image":"ghcr.io/team/base:1"}"#, "image"),
            (r#"{"command":"agent --serve"}"#, "command"),
            (r#"{"workdir":"/workspace"}"#, "workdir"),
            (r#"{"user":"node"}"#, "user"),
            (r#"{"resources":{"cpu":2}}"#, "resources"),
        ] {
            let err = parse_mixin(&mixin_json(spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains(&format!("a mixin must not declare {block}")),
                "a block the sandbox owns must be refused by name rather than silently ignored; got: {err:#}"
            );
        }
    }

    #[test]
    fn a_mixin_may_not_name_a_connector() {
        let err = parse_mixin(&mixin_json(r#"{"connectors":["some-provider"]}"#)).unwrap_err();
        assert!(
            format!("{err:#}").contains("a mixin must not name a connector"),
            "how a credential is obtained is the user's decision on their own machine, and the retired field stays a sandbox-only divergence rather than spreading to a new kind; got: {err:#}"
        );
    }

    #[test]
    fn a_mixin_reports_its_forbidden_block_before_that_block_is_validated() {
        let err = parse_mixin(&mixin_json(r#"{"workdir":"relative/path"}"#)).unwrap_err();
        assert!(
            format!("{err:#}").contains("a mixin must not declare workdir"),
            "the author needs the answer that matters — the block does not belong here at all — not a complaint about its contents; got: {err:#}"
        );
    }

    #[test]
    fn a_mixin_needs_no_image_of_its_own() {
        assert!(
            parse_mixin(&mixin_json(r#"{"tools":["postgresql@17"]}"#)).is_ok(),
            "a mixin layers onto a sandbox, so the image requirement is the sandbox's alone"
        );
    }

    #[test]
    fn each_reader_answers_only_for_its_own_kind() {
        let sandbox_err = parse_mixin(&def_json(r#"{"image":"x:1"}"#)).unwrap_err();
        assert!(
            format!("{sandbox_err:#}").contains("expected kind mixin"),
            "got: {sandbox_err:#}"
        );
        let mixin_err = parse(&mixin_json(r#"{"tools":["postgresql@17"]}"#)).unwrap_err();
        assert!(
            format!("{mixin_err:#}").contains("expected kind sandbox"),
            "got: {mixin_err:#}"
        );
    }

    #[test]
    fn a_document_carries_its_name_above_the_spec() {
        let def = parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"reviewer","spec":{"image":"x:1"}}"#,
        )
        .expect("§2 puts identity above spec and content inside it");
        assert_eq!(def.name, "reviewer");
        let err = parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","metadata":{"name":"reviewer"},"spec":{"image":"x:1"}}"#,
        )
        .expect_err("the old nesting is gone rather than accepted alongside it");
        assert!(
            format!("{err:#}").contains("metadata"),
            "the refusal has to name the key it did not know; got: {err:#}"
        );
    }

    #[test]
    fn a_document_names_its_egress_where_the_specification_names_it() {
        let def = parse(&def_json(
            r#"{"image":"x:1","egress":{"http":[{"match":"api.example.test","verdict":"allow"}]}}"#,
        ))
        .expect("§3.1's field table names this block `egress`, not `policy`");
        assert_eq!(def.spec.egress.http.len(), 1);
        let err = parse(&def_json(
            r#"{"image":"x:1","policy":{"egress":{"http":[]}}}"#,
        ))
        .expect_err("the old spelling is gone rather than accepted alongside the new one");
        assert!(
            format!("{err:#}").contains("policy"),
            "the refusal has to name the key it did not know; got: {err:#}"
        );
    }

    #[test]
    fn a_document_declares_the_mixins_it_layers_on() {
        let def = parse(&def_json(&format!(
            r#"{{"image":"x:1","mixins":["./mixins/postgres-tools/","{PINNED}"]}}"#
        )))
        .expect("a local directory and a digest-pinned reference are both legal");
        assert_eq!(def.spec.mixins.len(), 2);
        let mixin_of_a_mixin = parse_mixin(&mixin_json(&format!(r#"{{"mixins":["{PINNED}"]}}"#)))
            .expect("a mixin may build on other mixins, exactly as a sandbox does");
        assert_eq!(mixin_of_a_mixin.spec.mixins.len(), 1);
    }

    #[test]
    fn a_remote_mixin_reference_that_is_not_digest_pinned_is_refused() {
        for reference in [
            "ghcr.io/acme/postgres-tools:1.4.0",
            "ghcr.io/acme/postgres-tools",
            "ghcr.io/acme/postgres-tools@sha256:tooshort",
        ] {
            let err = parse(&def_json(&format!(
                r#"{{"image":"x:1","mixins":["{reference}"]}}"#
            )))
            .unwrap_err();
            assert!(
                format!("{err:#}").contains("must be digest-pinned")
                    && format!("{err:#}").contains("./"),
                "a published sandbox has to resolve to the same thing for everyone, so a tag that can move is refused; {reference}: got {err:#}"
            );
        }
    }

    #[test]
    fn validation_answers_for_the_kind_the_file_declares() {
        assert!(
            validate(&mixin_json(r#"{"tools":["postgresql@17"]}"#)).is_ok(),
            "a mixin validates offline like any other document"
        );
        let err = validate(&mixin_json(r#"{"image":"ghcr.io/team/base:1"}"#)).unwrap_err();
        assert!(
            format!("{err:#}").contains("a mixin must not declare image"),
            "routing on the declared kind is what lets the mixin rule report itself; got: {err:#}"
        );
    }

    #[test]
    fn a_mixin_entry_naming_nothing_is_refused() {
        let err = parse(&def_json(r#"{"image":"x:1","mixins":["  "]}"#)).unwrap_err();
        assert!(
            format!("{err:#}").contains("must name a directory or an OCI reference"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_reads_declared_tools() {
        let json = def_json(r#"{"image":"ghcr.io/team/base:1","tools":["node@22","python@3.12"]}"#);
        let def = parse(&json).unwrap();
        assert_eq!(
            def.spec.tools,
            vec!["node@22".to_string(), "python@3.12".to_string()]
        );
    }

    #[test]
    fn parse_defaults_tools_to_empty() {
        let def = parse(&def_json(r#"{"image":"ghcr.io/team/base:1"}"#)).unwrap();
        assert!(def.spec.tools.is_empty());
    }

    #[test]
    fn parse_requires_a_tool_version() {
        let err = parse(&def_json(
            r#"{"image":"ghcr.io/team/base:1","tools":["node"]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains(r#"explicit version such as "node@22" or "node@latest""#),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_malformed_tool_entry() {
        let err = parse(&def_json(
            r#"{"image":"ghcr.io/team/base:1","tools":["node@"]}"#,
        ))
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains(r#""node@""#) && msg.contains(r#""name@version""#),
            "got: {msg}"
        );
    }

    #[test]
    fn parse_rejects_an_engine_prefixed_tool() {
        let err = parse(&def_json(
            r#"{"image":"ghcr.io/team/base:1","tools":["aqua:node@22"]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("engine backend prefix"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_reads_the_whole_flat_definition() {
        let json = def_json(
            r#"{"image":"ghcr.io/team/base:1","command":"agent --serve","workdir":"/workspace","env":{"MODE":"research"},"resources":{"cpu":2,"memory":"1Gi"},"egress":{"http":[{"match":"api.example.test","verdict":"allow"},{"match":"*","verdict":"deny"}]},"connectors":["some-provider"],"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000"}],"volumes":[{"type":"bind","source":".","target":"/workspace"},{"type":"volume","source":"home","target":"/root/.home","readOnly":true}],"ports":[{"container":8080}]}"#,
        );
        let def = parse(&json).unwrap();
        assert_eq!(def.name, "hermes");
        assert_eq!(def.spec.image, "ghcr.io/team/base:1");
        assert_eq!(def.spec.command.as_deref(), Some("agent --serve"));
        assert_eq!(def.spec.workdir.as_deref(), Some("/workspace"));
        assert_eq!(
            def.spec.env.get("MODE").map(String::as_str),
            Some("research")
        );
        assert_eq!(
            def.spec.egress.http.last().unwrap().match_pattern,
            "*",
            "a closed baseline carries its lockdown as a catch-all deny"
        );
        assert_eq!(def.spec.egress.http.len(), 2);
        assert_eq!(def.spec.connectors, vec!["some-provider".to_string()]);
        assert_eq!(def.spec.credentials[0].env_var, "SOME_TOKEN");
        assert_eq!(def.spec.volumes[0].source(), ".");
        assert!(def.spec.volumes[0].is_bind());
        assert_eq!(def.spec.volumes[1].source(), "home");
        assert!(def.spec.volumes[1].read_only());
        assert_eq!(def.spec.ports[0].container, 8080);
    }

    #[test]
    fn parse_reads_an_omitted_policy_as_deciding_nothing() {
        let def = parse(&def_json(r#"{"image":"ghcr.io/team/base:1"}"#)).unwrap();
        assert!(def.spec.egress.http.is_empty());
        assert!(def.spec.egress.tcp.is_empty());
        assert!(def.spec.connectors.is_empty());
    }

    #[test]
    fn parse_rejects_an_upstream_route_transport() {
        let err = parse(&def_json(
            r#"{"image":"ghcr.io/team/base:1","egress":{"http":[{"match":"api.example.test","verdict":"allow","transport":"upstream"}]}}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("upstream transport isn't supported in the local sandbox"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_relative_binary_scope() {
        let err = parse(&def_json(
            r#"{"image":"ghcr.io/team/base:1","egress":{"http":[{"match":"git.example.test","verdict":"allow","binaries":["git"]}]}}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("is not an absolute path"),
            "a scope core would reject must fail here, not inside the guest: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_an_empty_binary_scope() {
        let err = parse(&def_json(
            r#"{"image":"ghcr.io/team/base:1","egress":{"http":[{"match":"git.example.test","verdict":"allow","binaries":[]}]}}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("matches no caller"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_accepts_an_absolute_binary_scope() {
        let def = parse(&def_json(
            r#"{"image":"ghcr.io/team/base:1","egress":{"http":[{"match":"git.example.test","verdict":"allow","binaries":["/usr/bin/git"]}]}}"#,
        ))
        .unwrap();
        assert_eq!(
            def.spec.egress.http[0].binaries,
            Some(vec!["/usr/bin/git".to_string()])
        );
    }

    #[test]
    fn parse_allows_a_floating_image_locally_pinning_is_a_publish_time_concern() {
        parse(&def_json(r#"{"image":"ghcr.io/team/base:1.4.0"}"#))
            .expect("a local definition may name a floating tag; publish pins it by digest");
    }

    #[test]
    fn parse_rejects_a_foreign_api_version() {
        let json = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"sandbox","name":"hermes","spec":{"image":"x:1"}}"#;
        let err = parse(json).unwrap_err();
        assert!(
            format!("{err:#}").contains("unexpected apiVersion"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_wrong_kind() {
        let json = br#"{"apiVersion":"lns.run/v1","kind":"Policy","name":"hermes","spec":{"image":"x:1"}}"#;
        let err = parse(json).unwrap_err();
        assert!(
            format!("{err:#}").contains("expected kind sandbox"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_an_invalid_name() {
        let json =
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"-bad","spec":{"image":"x:1"}}"#;
        let err = parse(json).unwrap_err();
        assert!(format!("{err:#}").contains("invalid name"), "got: {err:#}");
    }

    #[test]
    fn parse_requires_a_non_empty_image() {
        let err = parse(&def_json(r#"{"image":""}"#)).unwrap_err();
        assert!(
            format!("{err:#}").contains("must carry an image"),
            "got: {err:#}"
        );
        let err = parse(&def_json(r#"{}"#)).unwrap_err();
        assert!(
            format!("{err:#}").contains("must carry an image"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_traversing_volume_target() {
        let err = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"name":"home","target":"/root/../etc"}]}"#,
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("`..` segment"), "got: {err:#}");
    }

    #[test]
    fn parse_rejects_a_volume_mounted_into_the_lens_runtime_namespace() {
        for mount in ["/", "/.lens", "/.lens/guest-tools/bin"] {
            let spec =
                format!(r#"{{"image":"x:1","volumes":[{{"name":"home","target":"{mount}"}}]}}"#);
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("/.lens runtime namespace"),
                "volume {mount}: got: {err:#}"
            );
        }
    }

    #[test]
    fn parse_accepts_the_legacy_named_volume_shape() {
        let def = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"name":"home","target":"/root/.home","readOnly":true}]}"#,
        ))
        .unwrap();
        assert_eq!(def.spec.volumes[0].source(), "home");
        assert!(!def.spec.volumes[0].is_bind());
        assert!(def.spec.volumes[0].read_only());
    }

    #[test]
    fn parse_rejects_a_relative_workdir() {
        let err = parse(&def_json(r#"{"image":"x:1","workdir":"workspace"}"#)).unwrap_err();
        assert!(format!("{err:#}").contains("workdir"), "got: {err:#}");
        assert!(format!("{err:#}").contains("absolute"), "got: {err:#}");
    }

    #[test]
    fn parse_rejects_duplicate_mount_targets() {
        let err = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"type":"bind","source":".","target":"/workspace"},{"type":"volume","source":"cache","target":"/workspace"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate volume target /workspace"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_bind_source_that_escapes_the_project() {
        let err = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"type":"bind","source":"../outside","target":"/workspace"}]}"#,
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("bind source"), "got: {err:#}");
        assert!(format!("{err:#}").contains("`..`"), "got: {err:#}");
    }

    #[test]
    fn parse_rejects_ambiguous_or_invalid_volume_sources() {
        let cases = [
            (
                r#"{"type":"bind","name":"some-bind","target":"/data"}"#,
                "bind volume must use source",
            ),
            (
                r#"{"type":"volume","source":"some-cache","name":"other-cache","target":"/data"}"#,
                "either source or name",
            ),
            (
                r#"{"source":"some-cache","target":"/data"}"#,
                "must declare type",
            ),
            (
                r#"{"type":"bind","target":"/data"}"#,
                "bind source must not be empty",
            ),
            (
                r#"{"type":"bind","source":"project files","target":"/data"}"#,
                "must not contain whitespace",
            ),
            (
                r#"{"type":"volume","target":"/data"}"#,
                "named volume source must not be empty",
            ),
            (
                r#"{"type":"volume","source":".","target":"/data"}"#,
                "reserved",
            ),
            (
                r#"{"type":"volume","source":"some/cache","target":"/data"}"#,
                "character '/' not allowed",
            ),
        ];
        for (volume, expected) in cases {
            let spec = format!(r#"{{"image":"x:1","volumes":[{volume}]}}"#);
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains(expected),
                "expected {expected:?}, got: {err:#}"
            );
        }
    }

    #[test]
    fn parse_rejects_env_keys_that_would_produce_a_malformed_entry() {
        for spec in [
            r#"{"image":"x:1","env":{"FO=O":"v"}}"#,
            r#"{"image":"x:1","env":{"FOO\nBAR":"v"}}"#,
            r#"{"image":"x:1","env":{"FOO BAR":"v"}}"#,
            r#"{"image":"x:1","env":{"":"v"}}"#,
        ] {
            let err = parse(&def_json(spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("invalid env key"),
                "spec {spec}: got: {err:#}"
            );
        }
    }

    #[test]
    fn parse_rejects_a_non_positive_resource_request() {
        for spec in [
            r#"{"image":"x:1","resources":{"cpu":0}}"#,
            r#"{"image":"x:1","resources":{"cpu":-2}}"#,
            r#"{"image":"x:1","resources":{"memory":0}}"#,
        ] {
            let err = parse(&def_json(spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("must be a positive count or size"),
                "spec {spec}: got: {err:#}"
            );
        }
    }

    #[test]
    fn parse_rejects_a_memory_size_the_flag_would_also_reject() {
        for (spec, expected) in [
            (
                r#"{"image":"x:1","resources":{"memory":"38gg"}}"#,
                "unknown unit `gg`",
            ),
            (
                r#"{"image":"x:1","resources":{"memory":"12parsecs"}}"#,
                "unknown unit `parsecs`",
            ),
            (
                r#"{"image":"x:1","resources":{"memory":"lots"}}"#,
                "expected MiB",
            ),
            (
                r#"{"image":"x:1","resources":{"memory":"0Gi"}}"#,
                "at least 1 MiB",
            ),
        ] {
            let err = parse(&def_json(spec)).unwrap_err();
            let rendered = format!("{err:#}");
            assert!(
                rendered.contains("resources.memory") && rendered.contains(expected),
                "spec {spec}: got: {rendered}"
            );
        }
    }

    #[test]
    fn parse_reads_the_disk_a_sandbox_asks_for() {
        let def = parse(&def_json(r#"{"image":"x:1","resources":{"disk":"40Gi"}}"#)).unwrap();
        let disk = def.spec.resources.unwrap().disk.unwrap();
        assert_eq!(crate::disk::parse_bytes(&disk).unwrap(), 40 << 30);
    }

    #[test]
    fn parse_rejects_a_disk_the_service_could_not_provision() {
        for (spec, expected) in [
            (
                r#"{"image":"x:1","resources":{"disk":"50%"}}"#,
                "is a share",
            ),
            (
                r#"{"image":"x:1","resources":{"disk":"19Mi"}}"#,
                "at least 20Mi",
            ),
            (
                r#"{"image":"x:1","resources":{"disk":"16Ti"}}"#,
                "less than 16Ti",
            ),
            (
                r#"{"image":"x:1","resources":{"disk":"40parsecs"}}"#,
                "unknown unit `parsecs`",
            ),
        ] {
            let err = parse(&def_json(spec)).unwrap_err();
            let rendered = format!("{err:#}");
            assert!(
                rendered.contains("resources.disk") && rendered.contains(expected),
                "spec {spec}: got: {rendered}"
            );
        }
    }

    #[test]
    fn parse_reads_the_size_a_named_volume_asks_for() {
        let def = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"name":"cache","target":"/c","size":"100Gi"}]}"#,
        ))
        .unwrap();
        let size = def.spec.volumes[0].size.clone().unwrap();
        assert_eq!(crate::disk::parse_bytes(&size).unwrap(), 100 << 30);
    }

    #[test]
    fn a_volume_reports_the_size_it_declared_in_bytes() {
        let def = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"name":"cache","target":"/c","size":"40Gi"},{"name":"plain","target":"/p"}]}"#,
        ))
        .unwrap();
        assert_eq!(def.spec.volumes[0].size_bytes(), Some(40 << 30));
        assert_eq!(
            def.spec.volumes[1].size_bytes(),
            None,
            "a volume that declared nothing must stay distinguishable from one that declared the default"
        );
    }

    #[test]
    fn parse_rejects_a_size_on_a_bind_because_the_host_already_decided_it() {
        let err = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"type":"bind","source":".","target":"/w","size":"10Gi"}]}"#,
        ))
        .unwrap_err();
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("size applies to a named volume only"),
            "got: {rendered}"
        );
    }

    #[test]
    fn parse_rejects_a_volume_size_the_service_could_not_provision() {
        for (spec, expected) in [
            (r#""50%""#, "is a share"),
            (r#""19Mi""#, "at least 20Mi"),
            (r#""16Ti""#, "less than 16Ti"),
        ] {
            let json = format!(
                r#"{{"image":"x:1","volumes":[{{"name":"cache","target":"/c","size":{spec}}}]}}"#
            );
            let err = parse(&def_json(&json)).unwrap_err();
            let rendered = format!("{err:#}");
            assert!(
                rendered.contains("volumes[].size") && rendered.contains(expected),
                "spec {spec}: got: {rendered}"
            );
        }
    }

    #[test]
    fn parse_reads_a_declared_run_as_user() {
        let def = parse(&def_json(r#"{"image":"x:1","user":"root"}"#)).unwrap();
        assert_eq!(def.spec.user.as_deref(), Some("root"));

        let named = parse(&def_json(r#"{"image":"x:1","user":"node:staff"}"#)).unwrap();
        assert_eq!(named.spec.user.as_deref(), Some("node:staff"));

        let silent = parse(&def_json(r#"{"image":"x:1"}"#)).unwrap();
        assert_eq!(silent.spec.user, None);
    }

    #[test]
    fn parse_rejects_a_user_that_would_split_the_kernel_cmdline() {
        for spec in [
            r#"{"image":"x:1","user":"root sh"}"#,
            r#"{"image":"x:1","user":"a=b"}"#,
            r#"{"image":"x:1","user":""}"#,
            r#"{"image":"x:1","user":"   "}"#,
            r#"{"image":"x:1","user":"a:b:c"}"#,
            r#"{"image":"x:1","user":":staff"}"#,
            r#"{"image":"x:1","user":"node:"}"#,
            r#"{"image":"x:1","user":"\"root"}"#,
            r#"{"image":"x:1","user":"ro'ot"}"#,
        ] {
            let err = parse(&def_json(spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("user"),
                "the value reaches the guest on the space-joined kernel cmdline, so it must be screened here — spec {spec}: got {err:#}"
            );
        }
    }

    #[test]
    fn a_home_rooted_bind_source_parses() {
        let def = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"type":"bind","source":"~/.claude","target":"/home/agent/.claude"}]}"#,
        ))
        .unwrap();
        assert_eq!(
            def.spec.volumes[0].source(),
            "~/.claude",
            "the tilde travels verbatim; the CLI anchors it to this machine's home"
        );
    }

    #[test]
    fn a_user_relative_bind_source_is_refused() {
        for source in ["~alice/.claude", "~"] {
            let spec = format!(
                r#"{{"image":"x:1","volumes":[{{"type":"bind","source":"{source}","target":"/work"}}]}}"#
            );
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("only `~/` is supported"),
                "source {source} would otherwise bind a literal {source:?} directory under the project: got {err:#}"
            );
        }
    }

    #[test]
    fn a_home_rooted_bind_source_naming_a_secret_store_is_refused() {
        for source in [
            "~/.ssh",
            "~/.gnupg",
            "~/.aws",
            "~/.gnupg/private-keys-v1.d",
            "~/.ssh/id_rsa",
        ] {
            let spec = format!(
                r#"{{"image":"x:1","volumes":[{{"type":"bind","source":"{source}","target":"/work"}}]}}"#
            );
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("secret-shaped"),
                "`~/` is the first form that lets a pulled sandbox name the consumer's own home, and the bind's top-level scan never reaches a nested key store — {source}: got {err:#}"
            );
        }
    }

    #[test]
    fn an_absolute_bind_source_keeps_the_rules_it_already_had() {
        let def = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"type":"bind","source":"/srv/.ssh","target":"/work"}]}"#,
        ))
        .unwrap();
        assert_eq!(
            def.spec.volumes[0].source(),
            "/srv/.ssh",
            "an absolute source names the author's own machine and cannot travel; tightening it is a separate change"
        );
    }

    #[test]
    fn an_optional_bind_parses_and_defaults_to_required() {
        let def = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"type":"bind","source":"~/.claude","target":"/home/agent/.claude","optional":true},{"type":"bind","source":".","target":"/workspace"}]}"#,
        ))
        .unwrap();
        assert!(def.spec.volumes[0].optional());
        assert!(
            !def.spec.volumes[1].optional(),
            "a bind whose host path is missing must refuse the run unless the author opted out"
        );
    }

    #[test]
    fn optional_is_refused_on_a_named_volume() {
        let err = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"type":"volume","source":"cache","target":"/cache","optional":true}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("bind volume only"),
            "a named volume is created on demand, so it is never absent: got {err:#}"
        );
    }

    #[test]
    fn parse_accepts_an_exclude_list_on_a_bind_volume() {
        let def = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"type":"bind","source":".","target":"/workspace","exclude":[".cargo","tmp/scratch"]}]}"#,
        ))
        .unwrap();
        assert_eq!(def.spec.volumes[0].exclude(), [".cargo", "tmp/scratch"]);
    }

    #[test]
    fn parse_rejects_an_exclude_that_could_escape_the_bind_or_split_the_cmdline() {
        for entry in [
            "../secrets",
            "/etc/shadow",
            "a/../../etc",
            "./x",
            "",
            "two words",
            "quo\"te",
            ".cargo/",
            "a//b",
        ] {
            let spec = format!(
                r#"{{"image":"x:1","volumes":[{{"type":"bind","source":".","target":"/workspace","exclude":[{}]}}]}}"#,
                serde_json::to_string(entry).unwrap()
            );
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("exclude"),
                "entry {entry:?}: got {err:#}"
            );
        }
    }

    #[test]
    fn parse_rejects_an_exclude_on_a_named_volume() {
        let err = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"type":"volume","source":"cache","target":"/cache","exclude":[".git"]}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("bind volume only"),
            "got {err:#}"
        );
    }

    #[test]
    fn parse_accepts_documented_resource_shapes() {
        parse(&def_json(
            r#"{"image":"x:1","resources":{"cpu":2,"memory":"1Gi"}}"#,
        ))
        .unwrap();
        parse(&def_json(
            r#"{"image":"x:1","resources":{"cpu":"1500m","memory":"768Mi"}}"#,
        ))
        .unwrap();
        parse(&def_json(r#"{"image":"x:1","resources":{"memory":"640"}}"#)).unwrap();
        parse(&def_json(r#"{"image":"x:1","resources":{"memory":"2gi"}}"#)).unwrap();
        parse(&def_json(r#"{"image":"x:1","resources":{"memory":"2g"}}"#)).unwrap();
        parse(&def_json(r#"{"image":"x:1","resources":{"memory":512}}"#)).unwrap();
        parse(&def_json(r#"{"image":"x:1","resources":{"cpu":4}}"#)).unwrap();
        parse(&def_json(
            r#"{"image":"x:1","resources":{"cpu":"80%","memory":"80%"}}"#,
        ))
        .unwrap();
    }

    #[test]
    fn parse_rejects_a_relative_resource_request_outside_one_to_a_hundred() {
        for spec in [
            r#"{"image":"x:1","resources":{"memory":"0%"}}"#,
            r#"{"image":"x:1","resources":{"memory":"101%"}}"#,
            r#"{"image":"x:1","resources":{"memory":"%"}}"#,
            r#"{"image":"x:1","resources":{"cpu":"0%"}}"#,
            r#"{"image":"x:1","resources":{"cpu":"101%"}}"#,
        ] {
            parse(&def_json(spec)).unwrap_err();
        }
    }

    #[test]
    fn parse_rejects_an_out_of_range_port() {
        let err = parse(&def_json(
            r#"{"image":"x:1","ports":[{"container":70000}]}"#,
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("out of range"), "got: {err:#}");
    }

    #[test]
    fn parse_reads_a_path_fileset_entry() {
        let def = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"path":"./skills","guestPath":"/root/.agent/skills"}]}"#,
        ))
        .unwrap();
        assert_eq!(def.spec.filesets[0].path.as_deref(), Some("./skills"));
        assert_eq!(def.spec.filesets[0].guest_path, "/root/.agent/skills");
    }

    #[test]
    fn a_fileset_naming_another_artifact_is_no_longer_a_document_anything_reads() {
        let err = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"ref":"registry.example.test/team/settings@sha256:abc","guestPath":"/root/.agent/settings"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("ref"),
            "a fileset is not a separate artifact (docs/sandbox-spec.md §3.1.11), so strict decoding has to refuse the field rather than ignore it and run without those files; got: {err:#}"
        );
    }

    #[test]
    fn a_top_level_mount_key_is_refused() {
        let err = parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","mount":{"path":"/skills"},"spec":{"image":"x:1"}}"#,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("mount"),
            "identity sits above the spec and content inside it (§2), so a mount beside the name belongs to no grammar this reads; got: {err:#}"
        );
    }

    #[test]
    fn parse_reads_an_inline_fileset_without_changing_its_text() {
        let def = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"inline":{".claude/settings.json":"{\"enabled\":true}\n"},"guestPath":"/home/sandbox"}]}"#,
        ))
        .unwrap();
        let inline = def.spec.filesets[0].inline.as_ref().expect("inline source");
        assert_eq!(
            inline.get(".claude/settings.json").map(String::as_str),
            Some("{\"enabled\":true}\n")
        );
        assert_eq!(def.spec.filesets[0].owner, FilesetOwner::Workload);
    }

    #[test]
    fn parse_requires_exactly_one_fileset_source() {
        for entry in [
            r#"{"path":"./skills","inline":{"settings.json":"{}"},"guestPath":"/s"}"#,
            r#"{"inline":{},"guestPath":"/s"}"#,
        ] {
            let spec = format!(r#"{{"image":"x:1","filesets":[{entry}]}}"#);
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("exactly one of path, inline, or hostPath"),
                "got: {err:#}"
            );
        }
    }

    #[test]
    fn a_home_anchored_fileset_path_is_refused_for_the_reason_it_cannot_work() {
        for source in ["~/skills", "~", "~alice/skills"] {
            let spec =
                format!(r#"{{"image":"x:1","filesets":[{{"path":"{source}","guestPath":"/s"}}]}}"#);
            let message = format!("{:#}", parse(&def_json(&spec)).unwrap_err());
            assert!(
                message.contains("packed from a directory beside") && message.contains("hostPath"),
                "a packed path is never home-anchored, so the launch-time refusal claimed this machine has no home directory on a machine that plainly has one — {source}: got {message}"
            );
        }
    }

    #[test]
    fn a_host_path_fileset_parses_with_its_guest_path() {
        let def = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"hostPath":"~/.gitconfig","guestPath":"/home/agent/.gitconfig","optional":true}]}"#,
        ))
        .unwrap();
        assert_eq!(
            def.spec.filesets[0].host_path.as_deref(),
            Some("~/.gitconfig"),
            "the declared host path travels verbatim; expansion is a launch-time concern"
        );
        assert!(def.spec.filesets[0].optional);
    }

    #[test]
    fn a_host_path_fileset_is_required_unless_it_says_otherwise() {
        let def = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"hostPath":"/etc/gitconfig","guestPath":"/etc/gitconfig"}]}"#,
        ))
        .unwrap();
        assert!(
            !def.spec.filesets[0].optional,
            "an absent host path must refuse the run unless the author opted out"
        );
    }

    #[test]
    fn a_host_path_fileset_ships_the_field_under_its_camel_case_name() {
        let entry: FilesetEntry = serde_json::from_str(
            r#"{"hostPath":"~/.gitconfig","guestPath":"/home/agent/.gitconfig"}"#,
        )
        .expect("hostPath is the wire name; host_path would silently drop the source");
        assert_eq!(entry.host_path.as_deref(), Some("~/.gitconfig"));
    }

    #[test]
    fn a_fileset_setting_both_host_path_and_path_is_refused() {
        for entry in [
            r#"{"hostPath":"~/.gitconfig","path":"./skills","guestPath":"/s"}"#,
            r#"{"hostPath":"~/.gitconfig","inline":{"settings.json":"{}"},"guestPath":"/s"}"#,
        ] {
            let spec = format!(r#"{{"image":"x:1","filesets":[{entry}]}}"#);
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("exactly one of path, inline, or hostPath"),
                "got: {err:#}"
            );
        }
    }

    #[test]
    fn a_host_path_must_be_absolute_or_home_rooted() {
        for source in ["./x", "../x", "~", "~alice/x", "x", ""] {
            let spec = format!(
                r#"{{"image":"x:1","filesets":[{{"hostPath":{},"guestPath":"/s"}}]}}"#,
                serde_json::to_string(source).unwrap()
            );
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("hostPath"),
                "source {source:?}: got {err:#}"
            );
        }
        for source in ["/etc/gitconfig", "~/.gitconfig"] {
            let spec = format!(
                r#"{{"image":"x:1","filesets":[{{"hostPath":"{source}","guestPath":"/s"}}]}}"#
            );
            parse(&def_json(&spec)).unwrap_or_else(|e| panic!("source {source}: {e:#}"));
        }
    }

    #[test]
    fn a_user_relative_host_path_names_the_one_form_that_is_supported() {
        for source in ["~", "~alice/x"] {
            let spec = format!(
                r#"{{"image":"x:1","filesets":[{{"hostPath":"{source}","guestPath":"/s"}}]}}"#
            );
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("only `~/` is supported"),
                "source {source}: got {err:#}"
            );
        }
    }

    #[test]
    fn a_host_path_naming_a_secret_shaped_file_is_refused() {
        for source in ["~/.npmrc", "~/.ssh/id_rsa", "~/.aws/credentials", "/x/.env"] {
            let spec = format!(
                r#"{{"image":"x:1","filesets":[{{"hostPath":"{source}","guestPath":"/s"}}]}}"#
            );
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("secret-shaped"),
                "a hostPath file gets no KEEP/DROP prompt, so the name check is its only guard — {source}: got {err:#}"
            );
        }
    }

    #[test]
    fn a_host_path_that_could_split_the_guest_cmdline_is_refused() {
        for source in ["/etc/git config", "/etc/\"gitconfig", "/etc/../shadow"] {
            let spec = format!(
                r#"{{"image":"x:1","filesets":[{{"hostPath":{},"guestPath":"/s"}}]}}"#,
                serde_json::to_string(source).unwrap()
            );
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("hostPath"),
                "source {source:?}: got {err:#}"
            );
        }
    }

    #[test]
    fn a_host_path_guest_path_naming_a_directory_is_refused() {
        let err = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"hostPath":"~/.gitconfig","guestPath":"/home/agent/"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("names a guest file"),
            "a hostPath fileset copies one file to one guest path, so a directory guestPath has no meaning: {err:#}"
        );
    }

    #[test]
    fn optional_is_refused_on_a_fileset_without_a_host_path() {
        for entry in [
            r#"{"path":"./skills","guestPath":"/s","optional":true}"#,
            r#"{"inline":{"settings.json":"{}"},"guestPath":"/s","optional":true}"#,
        ] {
            let spec = format!(r#"{{"image":"x:1","filesets":[{entry}]}}"#);
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("optional applies to a hostPath fileset only"),
                "got: {err:#}"
            );
        }
    }

    #[test]
    fn a_host_path_fileset_still_collides_with_another_mount_target() {
        let err = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"name":"data","target":"/s"}],"filesets":[{"hostPath":"~/.gitconfig","guestPath":"/s"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate guest path /s"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_refuses_unsafe_and_secret_shaped_inline_paths() {
        for path in [
            "/etc/settings.json",
            "../settings.json",
            ".claude/../state.json",
            ".aws/credentials.json",
            "nested/.env.local",
        ] {
            let spec = format!(
                r#"{{"image":"x:1","filesets":[{{"inline":{{"{path}":"x"}},"guestPath":"/s"}}]}}"#
            );
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(format!("{err:#}").contains(path), "{path}: got {err:#}");
        }
    }

    #[test]
    fn parse_enforces_the_inline_path_length_cap() {
        let long_path = "a/".repeat(MAX_INLINE_PATH_BYTES / 2) + "f";
        let spec = format!(
            r#"{{"image":"x:1","filesets":[{{"inline":{{"{long_path}":"x"}},"guestPath":"/s"}}]}}"#
        );
        let err = parse(&def_json(&spec)).unwrap_err();
        assert!(
            format!("{err:#}").contains("4096-byte limit"),
            "got {err:#}"
        );
    }

    #[test]
    fn parse_enforces_the_inline_limit_per_file() {
        let accepted = "a".repeat(128 * 1024);
        let spec = serde_json::json!({
            "image": "x:1",
            "filesets": [{
                "inline": {"settings.json": accepted},
                "guestPath": "/s"
            }]
        });
        parse(&def_json(&spec.to_string())).unwrap();

        let oversized = "a".repeat(128 * 1024 + 1);
        let spec = serde_json::json!({
            "image": "x:1",
            "filesets": [{
                "inline": {"settings.json": oversized},
                "guestPath": "/s"
            }]
        });
        let err = parse(&def_json(&spec.to_string())).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("settings.json"), "got: {message}");
        assert!(message.contains("131072-byte limit"), "got: {message}");
        assert!(message.contains("path fileset"), "got: {message}");
    }

    #[test]
    fn parse_enforces_the_inline_total_bytes_cap_across_files() {
        let chunk = "a".repeat(128 * 1024);
        let at_cap: serde_json::Map<String, serde_json::Value> = (0..8)
            .map(|i| (format!("f{i}.txt"), chunk.clone().into()))
            .collect();
        let spec = serde_json::json!({
            "image": "x:1",
            "filesets": [{"inline": at_cap, "guestPath": "/s"}]
        });
        parse(&def_json(&spec.to_string())).unwrap();

        let over_cap: serde_json::Map<String, serde_json::Value> = (0..8)
            .map(|i| (format!("f{i}.txt"), chunk.clone().into()))
            .chain([("f8.txt".to_string(), "x".into())])
            .collect();
        let spec = serde_json::json!({
            "image": "x:1",
            "filesets": [{"inline": over_cap, "guestPath": "/s"}]
        });
        let err = parse(&def_json(&spec.to_string())).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("1048576-byte limit"), "got: {message}");
        assert!(message.contains("path fileset"), "got: {message}");
    }

    #[test]
    fn parse_enforces_the_inline_file_count_cap() {
        let at_cap: serde_json::Map<String, serde_json::Value> = (0..256)
            .map(|i| (format!("f{i}.txt"), "x".into()))
            .collect();
        let spec = serde_json::json!({
            "image": "x:1",
            "filesets": [{"inline": at_cap, "guestPath": "/s"}]
        });
        parse(&def_json(&spec.to_string())).unwrap();

        let over_cap: serde_json::Map<String, serde_json::Value> = (0..257)
            .map(|i| (format!("f{i}.txt"), "x".into()))
            .collect();
        let spec = serde_json::json!({
            "image": "x:1",
            "filesets": [{"inline": over_cap, "guestPath": "/s"}]
        });
        let err = parse(&def_json(&spec.to_string())).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("257 files"), "got: {message}");
        assert!(message.contains("256-file limit"), "got: {message}");
        assert!(message.contains("path fileset"), "got: {message}");
    }

    #[test]
    fn parse_defaults_fileset_owner_to_workload_so_seeded_state_is_rewritable() {
        let def = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"path":"./seed","guestPath":"/home/sandbox"}]}"#,
        ))
        .unwrap();
        assert_eq!(def.spec.filesets[0].owner, FilesetOwner::Workload);
    }

    #[test]
    fn parse_reads_an_explicit_root_owner_that_pins_shipped_inputs() {
        let def = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"path":"./skills","guestPath":"/opt/skills","owner":"root"}]}"#,
        ))
        .unwrap();
        assert_eq!(def.spec.filesets[0].owner, FilesetOwner::Root);
    }

    #[test]
    fn parse_rejects_an_unknown_fileset_owner() {
        let err = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"path":"./seed","guestPath":"/s","owner":"nobody"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("unknown variant"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_fileset_mounted_into_the_lens_runtime_namespace() {
        for mount in ["/", "/./", "/.lens", "/.lens/", "/.lens/bin"] {
            let spec = format!(
                r#"{{"image":"x:1","filesets":[{{"path":"./seed","guestPath":"{mount}"}}]}}"#
            );
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("/.lens runtime namespace"),
                "mount {mount}: got: {err:#}"
            );
        }
    }

    #[test]
    fn parse_rejects_a_fileset_naming_no_source() {
        let err = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"guestPath":"/s"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("exactly one of path, inline, or hostPath"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_fileset_guest_path_that_smuggles_a_control_char() {
        let result = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"path":"./seed","guestPath":"/.lens\n/x"}]}"#,
        ));
        assert!(
            result.is_err(),
            "a guestPath of `/.lens\\n/x` slips past overlaps_runtime_namespace (first segment `.lens\\n` != `.lens`) and injects a `/x`-rooted line into the workload-owned chown manifest; it must be refused"
        );
    }

    #[test]
    fn parse_rejects_a_relative_or_traversing_fileset_guest_path() {
        for mount in ["skills", "/root/../etc"] {
            let spec = format!(
                r#"{{"image":"x:1","filesets":[{{"path":"./skills","guestPath":"{mount}"}}]}}"#
            );
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("fileset guestPath"),
                "got: {err:#}"
            );
        }
    }

    #[test]
    fn parse_rejects_duplicate_fileset_guest_paths() {
        let err = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"path":"./a","guestPath":"/s"},{"path":"./b","guestPath":"/s"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate guest path /s"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_fileset_colliding_with_a_volume_target() {
        let err = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"name":"data","target":"/s"}],"filesets":[{"path":"./a","guestPath":"/s"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate guest path /s"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_an_empty_fileset_source() {
        let err = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"path":"","guestPath":"/s"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must not be empty"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_an_out_of_range_host_port() {
        let err = parse(&def_json(
            r#"{"image":"x:1","ports":[{"host":0,"container":3003}]}"#,
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("out of range"), "got: {err:#}");
    }

    #[test]
    fn parse_rejects_duplicate_host_ports() {
        let err = parse(&def_json(
            r#"{"image":"x:1","ports":[{"host":8080,"container":3000},{"host":8080,"container":3001}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate host port 8080"),
            "two container ports mapping to one host port collide at bind time: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_duplicate_container_ports() {
        let err = parse(&def_json(
            r#"{"image":"x:1","ports":[{"container":3003},{"host":8080,"container":3003}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate container port 3003"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_an_invalid_connector_id() {
        let err = parse(&def_json(r#"{"image":"x:1","connectors":["Bad_Id"]}"#)).unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid connector id"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_reads_a_credential_as_the_injection_contract_it_declares() {
        let def = parse(&def_json(
            r#"{"image":"x:1","credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000","injections":[{"kind":"bearer_header","domain":"api.some-provider.example"},{"kind":"api_key_header","domain":"api.some-oauth.example","header":"x-api-key"}]}]}"#,
        ))
        .unwrap();
        let credential = &def.spec.credentials[0];
        assert_eq!(credential.env_var, "SOME_TOKEN");
        assert_eq!(credential.placeholder, "some_LNSPLACEHOLDER0000");
        assert_eq!(
            credential.injections[0].kind,
            lns_spec::InjectionKind::BearerHeader
        );
        assert_eq!(credential.injections[0].domain, "api.some-provider.example");
        assert_eq!(
            credential.injections[1].header.as_deref(),
            Some("x-api-key")
        );
    }

    #[test]
    fn parse_reads_a_credential_that_declares_no_injection() {
        let def = parse(&def_json(
            r#"{"image":"x:1","credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000"}]}"#,
        ))
        .unwrap();
        assert!(
            def.spec.credentials[0].injections.is_empty(),
            "injections are optional: a credential the workload only reads travels nowhere"
        );
    }

    #[test]
    fn parse_rejects_two_credentials_sharing_an_env_var() {
        let err = parse(&def_json(
            r#"{"image":"x:1","credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000"},{"envVar":"SOME_TOKEN","placeholder":"other_LNSPLACEHOLDER0000"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate credential env var \"SOME_TOKEN\""),
            "nothing inside one document disambiguates two entries claiming one variable, so the second silently decides which secret the workload reads; got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_credential_env_var_that_would_produce_a_malformed_entry() {
        for spec in [
            r#"{"image":"x:1","credentials":[{"envVar":" ","placeholder":"some_LNSPLACEHOLDER0000"}]}"#,
            r#"{"image":"x:1","credentials":[{"envVar":"SOME_TOKEN=x","placeholder":"some_LNSPLACEHOLDER0000"}]}"#,
            r#"{"image":"x:1","credentials":[{"envVar":"SOME_TOKEN\nLD_PRELOAD","placeholder":"some_LNSPLACEHOLDER0000"}]}"#,
            r#"{"image":"x:1","credentials":[{"envVar":"SOME TOKEN","placeholder":"some_LNSPLACEHOLDER0000"}]}"#,
        ] {
            let err = parse(&def_json(spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("invalid credential env var"),
                "spec {spec}: got: {err:#}"
            );
        }
    }

    #[test]
    fn parse_rejects_a_placeholder_a_real_token_could_pass_for() {
        let err = parse(&def_json(
            r#"{"image":"x:1","credentials":[{"envVar":"SOME_TOKEN","placeholder":"sk-live-0123456789"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must self-identify as fake"),
            "a document that publishes to a registry carries its placeholder with it, so one that reads like a token is a secret one edit from being committed; got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_an_injection_that_names_no_domain() {
        let err = parse(&def_json(
            r#"{"image":"x:1","credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000","injections":[{"kind":"bearer_header","domain":"  "}]}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must name the domain"),
            "injection is domain-keyed, so an entry with no domain sends the secret nowhere and says nothing about where it may go; got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_header_on_a_kind_that_cannot_carry_one() {
        let err = parse(&def_json(
            r#"{"image":"x:1","credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000","injections":[{"kind":"bearer_header","domain":"api.some-provider.example","header":"x-api-key"}]}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("only an api_key_header injection carries a header name"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_an_api_key_header_injection_with_no_header_name() {
        let err = parse(&def_json(
            r#"{"image":"x:1","credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000","injections":[{"kind":"api_key_header","domain":"api.some-provider.example"}]}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("api_key_header injection must name the header"),
            "the proxy has no header to set, so the injection would silently do nothing; got: {err:#}"
        );
    }

    #[test]
    fn parse_surfaces_a_malformed_document_as_a_parse_error() {
        let no_name = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","spec":{"image":"x:1"}}"#;
        let err = parse(no_name).unwrap_err();
        assert!(
            format!("{err:#}").contains("parsing sandbox definition"),
            "got: {err:#}"
        );
    }

    #[test]
    fn versioned_sandbox_definitions_reject_unknown_fields_recursively() {
        let specs = [
            r#"{"image":"x:1","unexpected":true}"#,
            r#"{"image":"x:1","resources":{"cpu":1,"unexpected":true}}"#,
            r#"{"image":"x:1","egress":{"unexpected":true}}"#,
            r#"{"image":"x:1","egress":{"http":[{"match":"api.example.test","verdict":"allow","unexpected":true}]}}"#,
            r#"{"image":"x:1","egress":{"http":[{"match":"api.example.test","verdict":"allow","rules":[{"path":"/v1","unexpected":true}]}]}}"#,
            r#"{"image":"x:1","credentials":[{"envVar":"SOME_TOKEN","placeholder":"lns-fake","injectons":[]}]}"#,
            r#"{"image":"x:1","volumes":[{"name":"data","target":"/data","readOlny":true}]}"#,
            r#"{"image":"x:1","filesets":[{"path":"./skills","guestPath":"/skills","unexpected":true}]}"#,
            r#"{"image":"x:1","ports":[{"container":3003,"unexpected":true}]}"#,
        ];
        for spec in specs {
            let err = parse(&def_json(spec)).unwrap_err();
            assert!(format!("{err:#}").contains("unknown field"), "got: {err:#}");
        }

        let top_level = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"x:1"},"unexpected":true}"#;
        let err = parse(top_level).unwrap_err();
        assert!(format!("{err:#}").contains("unknown field"), "got: {err:#}");
    }
}
