use anyhow::{Context, Result, bail};
use lns_policy::NetworkPolicy;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

use crate::spec::{self, CredentialSlot, Metadata, Port, Resources};

pub const API_VERSION: &str = "lns.run/v1";
pub const KIND: &str = "Sandbox";
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

/// True when a document declares the user-facing `lns.run/v1` API group.
pub fn is_sandbox_definition(config_json: &[u8]) -> bool {
    #[derive(Deserialize)]
    struct ApiOnly {
        #[serde(rename = "apiVersion", default)]
        api_version: String,
    }
    serde_json::from_slice::<ApiOnly>(config_json)
        .map(|d| d.api_version == API_VERSION)
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub metadata: Metadata,
    pub spec: SandboxSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VolumeType {
    Bind,
    Volume,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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
}

impl Volume {
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
}

/// The whole sandbox in one document: the base image plus its config, env, embedded network policy, mounts, and the connector ids it needs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SandboxSpec {
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub resources: Option<Resources>,
    #[serde(default)]
    pub policy: NetworkPolicy,
    #[serde(default)]
    pub connectors: Vec<String>,
    #[serde(default)]
    pub credentials: Vec<CredentialSlot>,
    #[serde(default)]
    pub volumes: Vec<Volume>,
    #[serde(default)]
    pub filesets: Vec<FilesetEntry>,
    #[serde(default)]
    pub ports: Vec<Port>,
}

/// Files shipped inside the artifact: a local directory packed and digest-pinned at push (path), or a pre-published FileSet (ref), snapshot-mounted at mountPath.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilesetEntry {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(rename = "ref", default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub inline: Option<BTreeMap<String, String>>,
    #[serde(rename = "mountPath")]
    pub mount_path: String,
    #[serde(default)]
    pub owner: FilesetOwner,
}

/// Who owns the materialized files in the guest: the run-as workload user (so the workload can rewrite its own seeded state), or root (pinned inputs the workload must not touch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
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
    metadata: Metadata,
    #[serde(default)]
    spec: SandboxSpec,
}

/// Parse and cross-field-validate a `lns.run/v1` sandbox definition, offline.
pub fn parse(config_json: &[u8]) -> Result<Definition> {
    let doc: Doc = serde_json::from_slice(config_json).context("parsing sandbox definition")?;
    if doc.api_version != API_VERSION {
        bail!(
            "unexpected apiVersion {:?}; expected {API_VERSION}",
            doc.api_version
        );
    }
    if doc.kind != KIND {
        bail!(
            "expected kind {KIND} but definition declares {:?}",
            doc.kind
        );
    }
    if !spec::is_valid_name(&doc.metadata.name) {
        bail!("invalid metadata.name {:?}", doc.metadata.name);
    }
    if doc.spec.image.trim().is_empty() {
        bail!("sandbox must carry an image; it is the base OCI image the sandbox runs");
    }
    for key in doc.spec.env.keys() {
        if !is_valid_env_key(key) {
            bail!(
                "invalid env key {key:?}: env keys must be non-empty and free of '=', whitespace, and control characters"
            );
        }
    }
    if let Some(resources) = &doc.spec.resources {
        validate_resources(resources)?;
    }
    doc.spec
        .policy
        .validate_local_transport()
        .context("sandbox policy")?;
    if let Some(workdir) = &doc.spec.workdir {
        spec::validate_mount_path(workdir).context("workdir")?;
    }
    let mut targets = BTreeSet::new();
    for volume in &doc.spec.volumes {
        spec::validate_mount_path(&volume.target)
            .with_context(|| format!("volume targeting {}", volume.target))?;
        if overlaps_runtime_namespace(&volume.target) {
            bail!(
                "volume target {} overlaps the /.lens runtime namespace, which belongs to the sandbox itself",
                volume.target
            );
        }
        validate_volume(volume)?;
        if !targets.insert(&volume.target) {
            bail!("duplicate volume target {}", volume.target);
        }
    }
    for connector in &doc.spec.connectors {
        if !spec::is_valid_name(connector) {
            bail!("invalid connector id {connector:?}");
        }
    }
    let mut slot_connectors = BTreeSet::new();
    for slot in &doc.spec.credentials {
        if !spec::is_valid_name(&slot.name) {
            bail!("invalid credential connector id {:?}", slot.name);
        }
        // One connector discloses one env var: a consent card names the one it was asked about, so a second slot would inject that secret somewhere the developer never saw.
        if !slot_connectors.insert(&slot.name) {
            bail!("duplicate credential connector {:?}", slot.name);
        }
        if slot.env.trim().is_empty() {
            bail!(
                "credential {:?} must name the env var it is injected as",
                slot.name
            );
        }
        if !is_valid_env_key(&slot.env) {
            bail!(
                "invalid credential env key {:?}: env keys must be non-empty and free of '=', whitespace, and control characters",
                slot.env
            );
        }
    }
    for fileset in &doc.spec.filesets {
        validate_fileset(fileset)?;
        if !targets.insert(&fileset.mount_path) {
            bail!("duplicate mount target {}", fileset.mount_path);
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
    Ok(Definition {
        metadata: doc.metadata,
        spec: doc.spec,
    })
}

fn validate_fileset(fileset: &FilesetEntry) -> Result<()> {
    let source_count = usize::from(fileset.path.is_some())
        + usize::from(fileset.reference.is_some())
        + usize::from(fileset.inline.is_some());
    if source_count != 1 || fileset.inline.as_ref().is_some_and(BTreeMap::is_empty) {
        bail!(
            "fileset targeting {} must set exactly one of path, ref, or inline",
            fileset.mount_path
        );
    }
    if fileset.path.as_ref().is_some_and(String::is_empty) {
        bail!("fileset path must not be empty");
    }
    if fileset.reference.as_ref().is_some_and(String::is_empty) {
        bail!("fileset ref must not be empty");
    }
    if let Some(inline) = &fileset.inline {
        validate_inline_files(inline)?;
    }
    spec::validate_mount_path(&fileset.mount_path).context("fileset mountPath")?;
    if overlaps_runtime_namespace(&fileset.mount_path) {
        bail!(
            "fileset mountPath {} overlaps the /.lens runtime namespace, which belongs to the sandbox itself",
            fileset.mount_path
        );
    }
    Ok(())
}

fn validate_inline_files(inline: &BTreeMap<String, String>) -> Result<()> {
    if inline.len() > MAX_INLINE_FILES {
        bail!(
            "inline fileset has {} files, more than the {MAX_INLINE_FILES}-file limit; use a path or ref fileset",
            inline.len()
        );
    }
    let mut total_bytes: usize = 0;
    for (path, content) in inline {
        validate_inline_path(path)?;
        if content.len() > MAX_INLINE_FILE_BYTES {
            bail!(
                "inline file {path:?} exceeds the {MAX_INLINE_FILE_BYTES}-byte limit; use a path or ref fileset"
            );
        }
        total_bytes += content.len();
    }
    if total_bytes > MAX_INLINE_TOTAL_BYTES {
        bail!(
            "inline fileset totals {total_bytes} bytes, more than the {MAX_INLINE_TOTAL_BYTES}-byte limit; use a path or ref fileset"
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
            "inline file path {path:?} must be a safe relative path beneath its fileset mountPath"
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

fn is_valid_env_key(key: &str) -> bool {
    !key.is_empty()
        && !key
            .chars()
            .any(|c| c == '=' || c.is_control() || c.is_whitespace())
}

/// A cpu/memory request is a positive count or size — a bare integer ≥ 1 or a digits-then-unit string like `500m`/`2Gi` whose numeric part is non-zero — while the service's unit-aware resolver keeps ownership of the host ceiling and the fallback for anything else.
fn quantity_is_positive(quantity: &spec::Quantity) -> bool {
    match quantity {
        spec::Quantity::Int(n) => *n >= 1,
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
    for (field, quantity) in [("cpu", &resources.cpu), ("memory", &resources.memory)] {
        if let Some(quantity) = quantity
            && !quantity_is_positive(quantity)
        {
            bail!("resources.{field} {quantity:?} must be a positive count or size");
        }
    }
    Ok(())
}

fn validate_volume(volume: &Volume) -> Result<()> {
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

fn validate_bind_source(source: &str) -> Result<()> {
    if source.is_empty() {
        bail!("bind source must not be empty");
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
    Ok(())
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

/// Schema + cross-field guards for a sandbox definition.
pub fn validate(config_json: &[u8]) -> Result<()> {
    parse(config_json).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::{Transport, Verdict};

    fn def_json(spec: &str) -> Vec<u8> {
        format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"hermes"}},"spec":{spec}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn parse_reads_the_whole_flat_definition() {
        let json = def_json(
            r#"{"image":"ghcr.io/team/base:1","command":"agent --serve","workdir":"/workspace","env":{"MODE":"research"},"resources":{"cpu":2,"memory":"1Gi"},"policy":{"defaultVerdict":"deny","allowedRoutes":[{"match":"api.example.test","verdict":"allow"}]},"connectors":["some-provider"],"credentials":[{"name":"some-provider","env":"SOME_TOKEN"}],"volumes":[{"type":"bind","source":".","target":"/workspace"},{"type":"volume","source":"home","target":"/root/.home","readOnly":true}],"ports":[{"container":8080}]}"#,
        );
        let def = parse(&json).unwrap();
        assert_eq!(def.metadata.name, "hermes");
        assert_eq!(def.spec.image, "ghcr.io/team/base:1");
        assert_eq!(def.spec.command.as_deref(), Some("agent --serve"));
        assert_eq!(def.spec.workdir.as_deref(), Some("/workspace"));
        assert_eq!(
            def.spec.env.get("MODE").map(String::as_str),
            Some("research")
        );
        assert_eq!(def.spec.policy.default_verdict, Verdict::Deny);
        assert_eq!(def.spec.policy.allowed_routes.len(), 1);
        assert_eq!(def.spec.connectors, vec!["some-provider".to_string()]);
        assert_eq!(def.spec.credentials[0].env, "SOME_TOKEN");
        assert_eq!(def.spec.volumes[0].source(), ".");
        assert!(def.spec.volumes[0].is_bind());
        assert_eq!(def.spec.volumes[1].source(), "home");
        assert!(def.spec.volumes[1].read_only());
        assert_eq!(def.spec.ports[0].container, 8080);
    }

    #[test]
    fn parse_defaults_the_policy_to_ask_and_direct_when_omitted() {
        let def = parse(&def_json(r#"{"image":"ghcr.io/team/base:1"}"#)).unwrap();
        assert_eq!(def.spec.policy.default_verdict, Verdict::Ask);
        assert_eq!(def.spec.policy.default_transport, Transport::Direct);
        assert!(def.spec.connectors.is_empty());
    }

    #[test]
    fn parse_rejects_an_upstream_default_transport() {
        let err = parse(&def_json(
            r#"{"image":"ghcr.io/team/base:1","policy":{"defaultTransport":"upstream"}}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("upstream transport isn't supported in the local sandbox"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_an_upstream_route_transport() {
        let err = parse(&def_json(
            r#"{"image":"ghcr.io/team/base:1","policy":{"allowedRoutes":[{"match":"api.example.test","verdict":"allow","transport":"upstream"}]}}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("upstream transport isn't supported in the local sandbox"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_allows_a_floating_image_locally_pinning_is_a_publish_time_concern() {
        parse(&def_json(r#"{"image":"ghcr.io/team/base:1.4.0"}"#))
            .expect("a local definition may name a floating tag; publish pins it by digest");
    }

    #[test]
    fn parse_rejects_a_foreign_api_version() {
        let json = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"x:1"}}"#;
        let err = parse(json).unwrap_err();
        assert!(
            format!("{err:#}").contains("unexpected apiVersion"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_wrong_kind() {
        let json = br#"{"apiVersion":"lns.run/v1","kind":"Policy","metadata":{"name":"hermes"},"spec":{"image":"x:1"}}"#;
        let err = parse(json).unwrap_err();
        assert!(
            format!("{err:#}").contains("expected kind Sandbox"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_an_invalid_name() {
        let json = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"-bad"},"spec":{"image":"x:1"}}"#;
        let err = parse(json).unwrap_err();
        assert!(format!("{err:#}").contains("metadata.name"), "got: {err:#}");
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
            r#"{"image":"x:1","resources":{"memory":"lots"}}"#,
            r#"{"image":"x:1","resources":{"memory":"0Gi"}}"#,
        ] {
            let err = parse(&def_json(spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("must be a positive count or size"),
                "spec {spec}: got: {err:#}"
            );
        }
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
    fn parse_reads_path_and_ref_fileset_entries() {
        let def = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"path":"./skills","mountPath":"/root/.agent/skills"},{"ref":"registry.example.test/team/settings@sha256:abc","mountPath":"/root/.agent/settings"}]}"#,
        ))
        .unwrap();
        assert_eq!(def.spec.filesets[0].path.as_deref(), Some("./skills"));
        assert_eq!(def.spec.filesets[0].mount_path, "/root/.agent/skills");
        assert_eq!(
            def.spec.filesets[1].reference.as_deref(),
            Some("registry.example.test/team/settings@sha256:abc")
        );
    }

    #[test]
    fn parse_reads_an_inline_fileset_without_changing_its_text() {
        let def = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"inline":{".claude/settings.json":"{\"enabled\":true}\n"},"mountPath":"/home/sandbox"}]}"#,
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
            r#"{"path":"./skills","inline":{"settings.json":"{}"},"mountPath":"/s"}"#,
            r#"{"ref":"reg/skills@sha256:abc","inline":{"settings.json":"{}"},"mountPath":"/s"}"#,
            r#"{"inline":{},"mountPath":"/s"}"#,
        ] {
            let spec = format!(r#"{{"image":"x:1","filesets":[{entry}]}}"#);
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("exactly one of path, ref, or inline"),
                "got: {err:#}"
            );
        }
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
                r#"{{"image":"x:1","filesets":[{{"inline":{{"{path}":"x"}},"mountPath":"/s"}}]}}"#
            );
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(format!("{err:#}").contains(path), "{path}: got {err:#}");
        }
    }

    #[test]
    fn parse_enforces_the_inline_path_length_cap() {
        let long_path = "a/".repeat(MAX_INLINE_PATH_BYTES / 2) + "f";
        let spec = format!(
            r#"{{"image":"x:1","filesets":[{{"inline":{{"{long_path}":"x"}},"mountPath":"/s"}}]}}"#
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
                "mountPath": "/s"
            }]
        });
        parse(&def_json(&spec.to_string())).unwrap();

        let oversized = "a".repeat(128 * 1024 + 1);
        let spec = serde_json::json!({
            "image": "x:1",
            "filesets": [{
                "inline": {"settings.json": oversized},
                "mountPath": "/s"
            }]
        });
        let err = parse(&def_json(&spec.to_string())).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("settings.json"), "got: {message}");
        assert!(message.contains("131072-byte limit"), "got: {message}");
        assert!(message.contains("path or ref fileset"), "got: {message}");
    }

    #[test]
    fn parse_enforces_the_inline_total_bytes_cap_across_files() {
        let chunk = "a".repeat(128 * 1024);
        let at_cap: serde_json::Map<String, serde_json::Value> = (0..8)
            .map(|i| (format!("f{i}.txt"), chunk.clone().into()))
            .collect();
        let spec = serde_json::json!({
            "image": "x:1",
            "filesets": [{"inline": at_cap, "mountPath": "/s"}]
        });
        parse(&def_json(&spec.to_string())).unwrap();

        let over_cap: serde_json::Map<String, serde_json::Value> = (0..8)
            .map(|i| (format!("f{i}.txt"), chunk.clone().into()))
            .chain([("f8.txt".to_string(), "x".into())])
            .collect();
        let spec = serde_json::json!({
            "image": "x:1",
            "filesets": [{"inline": over_cap, "mountPath": "/s"}]
        });
        let err = parse(&def_json(&spec.to_string())).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("1048576-byte limit"), "got: {message}");
        assert!(message.contains("path or ref fileset"), "got: {message}");
    }

    #[test]
    fn parse_enforces_the_inline_file_count_cap() {
        let at_cap: serde_json::Map<String, serde_json::Value> = (0..256)
            .map(|i| (format!("f{i}.txt"), "x".into()))
            .collect();
        let spec = serde_json::json!({
            "image": "x:1",
            "filesets": [{"inline": at_cap, "mountPath": "/s"}]
        });
        parse(&def_json(&spec.to_string())).unwrap();

        let over_cap: serde_json::Map<String, serde_json::Value> = (0..257)
            .map(|i| (format!("f{i}.txt"), "x".into()))
            .collect();
        let spec = serde_json::json!({
            "image": "x:1",
            "filesets": [{"inline": over_cap, "mountPath": "/s"}]
        });
        let err = parse(&def_json(&spec.to_string())).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("257 files"), "got: {message}");
        assert!(message.contains("256-file limit"), "got: {message}");
        assert!(message.contains("path or ref fileset"), "got: {message}");
    }

    #[test]
    fn parse_defaults_fileset_owner_to_workload_so_seeded_state_is_rewritable() {
        let def = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"path":"./seed","mountPath":"/home/sandbox"}]}"#,
        ))
        .unwrap();
        assert_eq!(def.spec.filesets[0].owner, FilesetOwner::Workload);
    }

    #[test]
    fn parse_reads_an_explicit_root_owner_that_pins_shipped_inputs() {
        let def = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"path":"./skills","mountPath":"/opt/skills","owner":"root"}]}"#,
        ))
        .unwrap();
        assert_eq!(def.spec.filesets[0].owner, FilesetOwner::Root);
    }

    #[test]
    fn parse_rejects_an_unknown_fileset_owner() {
        let err = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"path":"./seed","mountPath":"/s","owner":"nobody"}]}"#,
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
                r#"{{"image":"x:1","filesets":[{{"path":"./seed","mountPath":"{mount}"}}]}}"#
            );
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("/.lens runtime namespace"),
                "mount {mount}: got: {err:#}"
            );
        }
    }

    #[test]
    fn parse_rejects_a_fileset_with_both_path_and_ref_or_neither() {
        for entry in [
            r#"{"path":"./skills","ref":"reg/skills@sha256:abc","mountPath":"/s"}"#,
            r#"{"mountPath":"/s"}"#,
        ] {
            let spec = format!(r#"{{"image":"x:1","filesets":[{entry}]}}"#);
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("exactly one of path, ref, or inline"),
                "got: {err:#}"
            );
        }
    }

    #[test]
    fn parse_rejects_a_fileset_mount_path_that_smuggles_a_control_char() {
        let result = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"path":"./seed","mountPath":"/.lens\n/x"}]}"#,
        ));
        assert!(
            result.is_err(),
            "a mountPath of `/.lens\\n/x` slips past overlaps_runtime_namespace (first segment `.lens\\n` != `.lens`) and injects a `/x`-rooted line into the workload-owned chown manifest; it must be refused"
        );
    }

    #[test]
    fn parse_rejects_a_relative_or_traversing_fileset_mount_path() {
        for mount in ["skills", "/root/../etc"] {
            let spec = format!(
                r#"{{"image":"x:1","filesets":[{{"path":"./skills","mountPath":"{mount}"}}]}}"#
            );
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("fileset mountPath"),
                "got: {err:#}"
            );
        }
    }

    #[test]
    fn parse_rejects_duplicate_fileset_mount_paths() {
        let err = parse(&def_json(
            r#"{"image":"x:1","filesets":[{"path":"./a","mountPath":"/s"},{"path":"./b","mountPath":"/s"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate mount target /s"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_fileset_colliding_with_a_volume_target() {
        let err = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"name":"data","target":"/s"}],"filesets":[{"path":"./a","mountPath":"/s"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate mount target /s"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_an_empty_fileset_source() {
        for entry in [
            r#"{"path":"","mountPath":"/s"}"#,
            r#"{"ref":"","mountPath":"/s"}"#,
        ] {
            let spec = format!(r#"{{"image":"x:1","filesets":[{entry}]}}"#);
            let err = parse(&def_json(&spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("must not be empty"),
                "got: {err:#}"
            );
        }
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
    fn parse_rejects_an_invalid_credential_slot_connector_id() {
        let err = parse(&def_json(
            r#"{"image":"x:1","credentials":[{"name":"Bad_Id","env":"SOME_TOKEN"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid credential connector id"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_two_credential_slots_naming_one_connector() {
        let err = parse(&def_json(
            r#"{"image":"x:1","credentials":[{"name":"some-provider","env":"SOME_TOKEN"},{"name":"some-provider","env":"OTHER_TOKEN"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate credential connector \"some-provider\""),
            "one connector cannot disclose two env vars: the consent card names one of them, so the second would inject the same secret somewhere the developer was never shown; got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_credential_slot_with_no_env_target() {
        let err = parse(&def_json(
            r#"{"image":"x:1","credentials":[{"name":"some-provider","env":" "}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("env var it is injected as"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_credential_env_key_that_would_produce_a_malformed_entry() {
        for spec in [
            r#"{"image":"x:1","credentials":[{"name":"some-provider","env":"SOME_TOKEN=x"}]}"#,
            r#"{"image":"x:1","credentials":[{"name":"some-provider","env":"SOME_TOKEN\nLD_PRELOAD"}]}"#,
            r#"{"image":"x:1","credentials":[{"name":"some-provider","env":"SOME TOKEN"}]}"#,
        ] {
            let err = parse(&def_json(spec)).unwrap_err();
            assert!(
                format!("{err:#}").contains("invalid credential env key"),
                "spec {spec}: got: {err:#}"
            );
        }
    }

    #[test]
    fn parse_reads_a_required_credential_slot() {
        let def = parse(&def_json(
            r#"{"image":"x:1","credentials":[{"name":"some-provider","env":"SOME_TOKEN","required":true}]}"#,
        ))
        .unwrap();
        assert!(def.spec.credentials[0].required);
    }

    #[test]
    fn parse_surfaces_a_malformed_document_as_a_parse_error() {
        let no_metadata = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","spec":{"image":"x:1"}}"#;
        let err = parse(no_metadata).unwrap_err();
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
            r#"{"image":"x:1","policy":{"unexpected":true}}"#,
            r#"{"image":"x:1","policy":{"allowedRoutes":[{"match":"api.example.test","verdict":"allow","unexpected":true}]}}"#,
            r#"{"image":"x:1","policy":{"allowedRoutes":[{"match":"api.example.test","verdict":"allow","rules":[{"path":"/v1","unexpected":true}]}]}}"#,
            r#"{"image":"x:1","credentials":[{"name":"some-provider","env":"SOME_TOKEN","requred":true}]}"#,
            r#"{"image":"x:1","volumes":[{"name":"data","target":"/data","readOlny":true}]}"#,
            r#"{"image":"x:1","filesets":[{"path":"./skills","mountPath":"/skills","unexpected":true}]}"#,
            r#"{"image":"x:1","ports":[{"container":3003,"unexpected":true}]}"#,
        ];
        for spec in specs {
            let err = parse(&def_json(spec)).unwrap_err();
            assert!(format!("{err:#}").contains("unknown field"), "got: {err:#}");
        }

        let top_level = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"x:1"},"unexpected":true}"#;
        let err = parse(top_level).unwrap_err();
        assert!(format!("{err:#}").contains("unknown field"), "got: {err:#}");

        let metadata = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes","unexpected":true},"spec":{"image":"x:1"}}"#;
        let err = parse(metadata).unwrap_err();
        assert!(format!("{err:#}").contains("unknown field"), "got: {err:#}");
    }

    #[test]
    fn is_sandbox_definition_detects_the_api_group() {
        assert!(is_sandbox_definition(&def_json(r#"{"image":"x:1"}"#)));
        assert!(!is_sandbox_definition(
            br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox"}"#
        ));
        assert!(!is_sandbox_definition(b"not json"));
    }
}
