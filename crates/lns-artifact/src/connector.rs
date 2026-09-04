//! The connector document — `docs/sandbox-spec.md` §3.2.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use lns_policy::Egress;
use serde::{Deserialize, Serialize};

use crate::sandbox::FilesetEntry;
use crate::spec;

/// The values each built-in `auth.kind` produces, which a credential's `field` names. A `code` method declares its own instead, and any other kind parses and leaves its method unofferable (§3.2.2).
const BUILT_IN_OUTPUTS: [(&str, &[&str]); 1] = [("token", &["token"])];

/// The kind whose mechanism the connector carries itself, so what it produces is the document's to say rather than this version's (§3.2.6).
const CODE: &str = "code";

/// What one method may write, counted across its filesets (§3.2.3).
pub const MAX_METHOD_FILESET_BYTES: usize = crate::sandbox::MAX_INLINE_TOTAL_BYTES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorDefinition {
    pub name: String,
    pub spec: ConnectorSpec,
}

/// Detection in `serves`, and the alternative ways to connect in `methods`.
///
/// `deny_unknown_fields` cannot ride with `flatten`, so `misplaced` collects what
/// this shape does not name and [`refuse_a_payload_block_outside_a_method`] holds
/// the line — which is what lets the message say where a block belongs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorSpec {
    #[serde(default)]
    pub serves: Vec<String>,
    #[serde(default)]
    pub methods: Vec<Method>,
    /// Every block a method owns, refused here so the message can say where it belongs (§3.2.2).
    #[serde(flatten)]
    misplaced: BTreeMap<String, serde_json::Value>,
}

/// One way to connect: an optional `auth`, plus the payload a grant applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Method {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<Auth>,
    #[serde(default)]
    pub egress: Egress,
    #[serde(default)]
    pub credentials: Vec<lns_spec::Credential>,
    #[serde(default)]
    pub filesets: Vec<FilesetEntry>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// A block a running guest cannot be given, kept only so the refusal can name the mechanism that decides it.
    #[serde(flatten)]
    refused: BTreeMap<String, serde_json::Value>,
}

impl Method {
    /// The card can offer this method only if this version implements its mechanism; one it does not is not an error (§3.2.2).
    pub fn is_offerable(&self) -> bool {
        self.auth.as_ref().is_none_or(Auth::is_implemented)
    }

    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
}

/// The auth output one credential draws its value from: the `field` it names, or the one value the method's `auth` produces. `None` under a kind this version does not know, whose method cannot be connected anyway, and `None` where the auth produces several and the credential named none (§4.1).
pub fn input_of(method: &Method, credential: &lns_spec::Credential) -> Option<String> {
    if let Some(field) = credential.field.clone() {
        return Some(field);
    }
    match method.auth.as_ref()?.outputs()?.as_slice() {
        [only] => Some((*only).to_string()),
        _ => None,
    }
}

/// How the user proves they may use a method. `kind` decides what else it accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Auth {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Everything the `kind` decides the shape of, left undecoded here so a kind this version does not know decodes nothing of its own (§3.2.2).
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

/// What a `code` auth carries beyond the fields every kind has (§3.2.6). Decoded only where the kind is `code`, so no other kind's spelling of these names is this version's to read.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeAuth {
    #[serde(default)]
    pub component: Option<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub exec: bool,
    #[serde(default)]
    limits: Option<Limits>,
}

impl CodeAuth {
    /// How long this component may take, which is the ceiling wherever the document left it out (§3.2.6).
    pub fn limits(&self) -> Limits {
        self.limits.clone().unwrap_or_default()
    }
}

/// The longest §3.2.6 lets one call and one session take, and what a method leaving them out gets.
const MAX_CALL_SECONDS: u32 = 30;
const MAX_SESSION_SECONDS: u32 = 900;

/// How long a component may take. Absent means the ceiling (§3.2.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    #[serde(default = "max_call_seconds")]
    pub call_seconds: u32,
    #[serde(default = "max_session_seconds")]
    pub session_seconds: u32,
}

fn max_call_seconds() -> u32 {
    MAX_CALL_SECONDS
}

fn max_session_seconds() -> u32 {
    MAX_SESSION_SECONDS
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            call_seconds: MAX_CALL_SECONDS,
            session_seconds: MAX_SESSION_SECONDS,
        }
    }
}

/// The names only a `code` auth may carry, so a refusal on another kind can say which one was misplaced (§5).
const CODE_ONLY_FIELDS: [&str; 5] = ["component", "outputs", "hosts", "exec", "limits"];

impl Auth {
    /// The `code` block this auth carries, decoded strictly. `None` for every other kind, and an error where the kind is `code` and the block will not read.
    pub fn code(&self) -> Option<Result<CodeAuth>> {
        if self.kind != CODE {
            return None;
        }
        let block = serde_json::Value::Object(self.extra.clone().into_iter().collect());
        Some(serde_json::from_value(block).context("reading a code auth"))
    }

    /// The values this kind produces, or `None` for a kind this version does not know and so decodes nothing of (§3.2.2).
    fn outputs(&self) -> Option<Vec<String>> {
        if let Some(code) = self.code() {
            return code.ok().map(|code| code.outputs);
        }
        BUILT_IN_OUTPUTS
            .iter()
            .find(|(kind, _)| *kind == self.kind)
            .map(|(_, outputs)| outputs.iter().map(|o| (*o).to_string()).collect())
    }

    /// Whether this build can actually run this mechanism. `code` parses and validates here, but no component runtime exists yet, so offering it would ask the user to hand-type what a component is supposed to produce.
    fn is_implemented(&self) -> bool {
        BUILT_IN_OUTPUTS.iter().any(|(kind, _)| *kind == self.kind)
    }

    /// Which `code`-only name this auth carries, for a kind that may not carry one.
    fn code_only_field(&self) -> Option<&str> {
        self.extra
            .keys()
            .find(|key| CODE_ONLY_FIELDS.contains(&key.as_str()))
            .map(String::as_str)
    }

    /// What a connect calls the value it asks for: the author's own word for it, else the kind.
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.kind)
    }
}

/// The blocks a method may not carry, each naming the mechanism that decides it rather than the rule.
const REFUSED_BLOCKS: [(&str, &str); 10] = [
    (
        "tools",
        "a connector is applied to a guest that is already running, and tools are installed before the guest boots",
    ),
    (
        "scripts",
        "a connector is applied to a guest that is already running, and a script runs once before the workload — connecting a connector must not mean running code",
    ),
    (
        "volumes",
        "which mounts exist is fixed when the guest is created, and a connector is applied to a guest that is already running",
    ),
    (
        "ports",
        "which listeners exist is fixed when the guest is created, and a connector is applied to a guest that is already running",
    ),
    (
        "mixins",
        "the card shows one document, and a graph would apply egress and credentials from documents the user never saw",
    ),
    ("image", "it describes one launch, and the sandbox owns it"),
    (
        "command",
        "it describes one launch, and the sandbox owns it",
    ),
    (
        "workdir",
        "it describes one launch, and the sandbox owns it",
    ),
    ("user", "it describes one launch, and the sandbox owns it"),
    (
        "resources",
        "it describes one launch, and the sandbox owns it",
    ),
];

/// Every block a method owns, so `spec` carrying one can point at where it belongs.
const METHOD_BLOCKS: [&str; 4] = ["egress", "credentials", "filesets", "env"];

/// Parse and cross-field-validate a `lns.run/v1` connector against the document alone; a caller holding the bytes of a `path` fileset passes them to [`parse_with_path_files`] instead.
pub fn parse(config_json: &[u8]) -> Result<ConnectorDefinition> {
    parse_with_path_files(config_json, &BTreeMap::new())
}

/// [`parse`], plus the §3.2.5 read of every `path` fileset the caller could read beside the document, keyed by the `path` the entry writes.
pub fn parse_with_path_files(
    config_json: &[u8],
    path_files: &BTreeMap<String, BTreeMap<String, Vec<u8>>>,
) -> Result<ConnectorDefinition> {
    let doc = spec::parse_envelope(config_json, spec::Kind::Connector)?;
    let connector: ConnectorSpec =
        serde_json::from_value(doc.spec).context("parsing connector spec")?;
    validate(&connector, path_files)?;
    Ok(ConnectorDefinition {
        name: doc.name,
        spec: connector,
    })
}

fn validate(
    connector: &ConnectorSpec,
    path_files: &BTreeMap<String, BTreeMap<String, Vec<u8>>>,
) -> Result<()> {
    refuse_a_payload_block_outside_a_method(connector)?;
    validate_serves(&connector.serves)?;
    if connector.methods.is_empty() {
        bail!("a connector must declare at least one method: methods are how it is connected");
    }
    let mut named = BTreeSet::new();
    for method in &connector.methods {
        validate_method(method, path_files)
            .with_context(|| format!("connector method {}", method.name))?;
        if !named.insert(&method.name) {
            bail!("duplicate connector method {:?}", method.name);
        }
    }
    refuse_one_variable_carrying_two_markers(connector)
}

/// §4.1: a run moves between methods and `env` reaches only later workloads, so the methods that share a variable must agree on the marker its live sessions hold.
fn refuse_one_variable_carrying_two_markers(connector: &ConnectorSpec) -> Result<()> {
    let mut marker: BTreeMap<&str, &str> = BTreeMap::new();
    for credential in connector
        .methods
        .iter()
        .flat_map(|method| &method.credentials)
    {
        let Some(variable) = credential.env_var.as_deref() else {
            continue;
        };
        let held = marker.entry(variable).or_insert(&credential.placeholder);
        if *held != credential.placeholder {
            bail!(
                "{variable} carries two placeholders across this connector's methods: a run that moves to the other method leaves its live sessions holding the first marker"
            );
        }
    }
    Ok(())
}

fn refuse_a_payload_block_outside_a_method(connector: &ConnectorSpec) -> Result<()> {
    if let Some(block) = METHOD_BLOCKS
        .iter()
        .find(|block| connector.misplaced.contains_key(**block))
    {
        bail!(
            "a connector must declare {block} inside a method, not beside methods: a grant applies one method, so a block outside them belongs to nothing"
        );
    }
    if let Some((block, why)) = REFUSED_BLOCKS
        .iter()
        .find(|(block, _)| connector.misplaced.contains_key(*block))
    {
        bail!("a connector must not declare {block}: {why}");
    }
    if let Some(unknown) = connector.misplaced.keys().next() {
        bail!("unknown field {unknown:?} in connector spec");
    }
    Ok(())
}

fn validate_serves(serves: &[String]) -> Result<()> {
    if serves.is_empty() {
        bail!(
            "a connector must declare at least one serves entry: serves is what decides when it is offered"
        );
    }
    for pattern in serves {
        if pattern.trim().is_empty() {
            bail!("a serves entry must name a destination");
        }
        if pattern.chars().any(char::is_whitespace) {
            bail!("serves entry {pattern:?} must not contain whitespace");
        }
        if let Some(why) = lns_policy::matching::unusable_port(pattern) {
            bail!(
                "serves entry {pattern:?}: {why}. lns reads the whole entry as one host name, so this connector is never offered"
            );
        }
    }
    Ok(())
}

fn validate_method(
    method: &Method,
    path_files: &BTreeMap<String, BTreeMap<String, Vec<u8>>>,
) -> Result<()> {
    if !spec::is_valid_name(&method.name) {
        bail!("invalid method name {:?}", method.name);
    }
    if let Some((block, why)) = REFUSED_BLOCKS
        .iter()
        .find(|(block, _)| method.refused.contains_key(*block))
    {
        bail!("a connector must not declare {block}: {why}");
    }
    if let Some(unknown) = method.refused.keys().next() {
        bail!("unknown field {unknown:?} in connector method");
    }
    if let Some(auth) = &method.auth {
        validate_auth(auth)?;
    } else if !method.credentials.is_empty() {
        bail!(
            "a method that declares credentials must declare auth: nothing would produce the value, and the credential would ship permanently unarmed"
        );
    }
    lns_spec::credential::validate_all(&method.credentials, lns_spec::credential::Source::Method)
        .map_err(anyhow::Error::msg)?;
    refuse_a_field_the_auth_does_not_produce(method)?;
    for key in method.env.keys() {
        if !lns_spec::is_legal_env_var_name(key) {
            bail!(
                "invalid env key {key:?}: env keys must be non-empty and free of '=', whitespace, and control characters"
            );
        }
    }
    lns_spec::credential::refuse_a_variable_a_credential_also_fills(
        method.env.keys(),
        &method.credentials,
    )
    .map_err(anyhow::Error::msg)?;
    method
        .egress
        .validate_local_transport()
        .context("connector policy")?;
    method
        .egress
        .validate_binary_scopes()
        .context("connector policy")?;
    let mut written = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut fileset_bytes = 0usize;
    for fileset in &method.filesets {
        crate::sandbox::validate_fileset(fileset, crate::spec::GuestAnchor::Home)?;
        if fileset.host_path.is_some() {
            bail!(
                "a connector must not declare a fileset hostPath: a connector is installed once and used in every project, so reading a path off whichever machine happens to be running it is a sandbox concern"
            );
        }
        // Per method, not per document: methods are alternatives, so only one ever writes.
        if !written.insert(guest_directory(&fileset.guest_path)) {
            bail!("duplicate guest path {}", fileset.guest_path);
        }
        for name in fileset.inline.iter().flatten().map(|(name, _)| name) {
            let path = guest_file(&fileset.guest_path, name);
            if !files.insert(path.clone()) {
                bail!(
                    "two of this method's files land on {path}: the guest can hold one, so a grant would write neither"
                );
            }
        }
        fileset_bytes += fileset
            .inline
            .iter()
            .flat_map(|files| files.values())
            .map(String::len)
            .sum::<usize>();
        fileset_bytes += fileset
            .path
            .as_ref()
            .and_then(|path| path_files.get(path))
            .map(|files| files.values().map(Vec::len).sum::<usize>())
            .unwrap_or_default();
    }
    if fileset_bytes > MAX_METHOD_FILESET_BYTES {
        bail!(
            "this method's filesets total {fileset_bytes} bytes, more than the {MAX_METHOD_FILESET_BYTES}-byte limit: a granted method's files are sent again on every policy change, and every connector a project grants shares one budget"
        );
    }
    refuse_a_secret_shaped_file_carrying_no_declared_placeholder(method, path_files)
}

/// One spelling per guest file. An empty or `.` segment is legal in a `guestPath`, so two spellings of one file would slip past every rule that compares paths.
pub fn guest_file(guest_path: &str, name: &str) -> String {
    guest_directory(&format!("{guest_path}/{name}"))
}

/// Home-anchored, like every connector `guestPath` (§3.2.3): the result always starts `~/`.
pub fn guest_directory(guest_path: &str) -> String {
    let mut resolved = String::from("~");
    for segment in guest_path
        .trim_start_matches('~')
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
    {
        resolved.push('/');
        resolved.push_str(segment);
    }
    resolved
}

/// §4.1: `field` names one of the method's `auth` outputs. A `field` naming anything else looks up a value nothing supplies, so the credential would install, connect and grant and still reach the guest unarmed.
fn refuse_a_field_the_auth_does_not_produce(method: &Method) -> Result<()> {
    let Some(auth) = &method.auth else {
        return Ok(());
    };
    // A kind this version does not know decodes no `auth`, so what it produces is not this version's to judge (§3.2.2).
    let Some(outputs) = auth.outputs() else {
        return Ok(());
    };
    let produced = outputs.join(", ");
    for credential in &method.credentials {
        match &credential.field {
            Some(field) if !outputs.contains(field) => bail!(
                "credential {} draws on field {field:?}, but a {:?} auth produces {produced}: nothing would supply that value and the credential would reach the guest unarmed",
                credential.owner(),
                auth.kind
            ),
            // §4.1 lets a credential name no field only where the auth produces one value.
            None if outputs.len() > 1 => bail!(
                "credential {} names no field, but a {:?} auth produces {produced}: nothing says which of them arms it, and the credential would reach the guest unarmed",
                credential.owner(),
                auth.kind
            ),
            _ => {}
        }
    }
    Ok(())
}

fn validate_auth(auth: &Auth) -> Result<()> {
    if auth.kind.trim().is_empty() {
        bail!("a method's auth must name its kind");
    }
    if let Some(code) = auth.code() {
        return validate_code_auth(&code?);
    }
    // A kind this version does not know decodes nothing of its auth, so neither its shape nor its field names are this version's to judge (§3.2.2).
    if !auth.is_implemented() {
        return Ok(());
    }
    if let Some(field) = auth.code_only_field() {
        bail!(
            "a {:?} auth declares {field}, which only a {CODE:?} auth carries: lns implements this mechanism itself, so what it produces and what it may reach are not the document's to say",
            auth.kind
        );
    }
    if let Some(unknown) = auth.extra.keys().next() {
        bail!(
            "unknown field {unknown:?} in a {:?} auth: strict decoding holds for a kind this version knows",
            auth.kind
        );
    }
    Ok(())
}

/// §3.2.6: a component produces whatever it says it produces, so the document must say it and lns must be able to hold it to that.
fn validate_code_auth(code: &CodeAuth) -> Result<()> {
    match code.component.as_deref() {
        None | Some("") => bail!(
            "a {CODE:?} auth must name its component: it is the implementation the method connects with"
        ),
        Some(component) => validate_component_path(component)?,
    }
    if code.outputs.is_empty() {
        bail!(
            "a {CODE:?} auth must declare outputs: a component produces whatever it says it produces, and without that list a credential's field cannot be checked before the connector is installed"
        );
    }
    let mut named = BTreeSet::new();
    for output in &code.outputs {
        if !named.insert(output) {
            bail!("a {CODE:?} auth declares the output {output:?} twice");
        }
    }
    for host in &code.hosts {
        validate_component_host(host)?;
    }
    let limits = code.limits();
    if limits.call_seconds > MAX_CALL_SECONDS {
        bail!(
            "a {CODE:?} auth gives one call {} seconds, more than the {MAX_CALL_SECONDS}-second ceiling: a person is waiting on a connect, and lns stops a component that outstays it",
            limits.call_seconds
        );
    }
    if limits.session_seconds > MAX_SESSION_SECONDS {
        bail!(
            "a {CODE:?} auth gives one session {} seconds, more than the {MAX_SESSION_SECONDS}-second ceiling: state a component holds between calls is secret material lns keeps in memory",
            limits.session_seconds
        );
    }
    Ok(())
}

/// §3.2.6: the card names these and lns enforces them per call, so an entry no matcher can read is a bound that silently holds nothing.
fn validate_component_host(host: &str) -> Result<()> {
    if host.trim().is_empty() {
        bail!("a {CODE:?} auth's hosts entry must name a destination");
    }
    if host.chars().any(char::is_whitespace) {
        bail!("hosts entry {host:?} must not contain whitespace");
    }
    if let Some(why) = lns_policy::matching::unusable_port(host) {
        bail!(
            "hosts entry {host:?}: {why}. lns reads the whole entry as one host name, so the component could never reach it"
        );
    }
    Ok(())
}

/// §3.2.6: the component is packed at publish, so it names one file beside the document and never one on whichever machine runs it.
fn validate_component_path(component: &str) -> Result<()> {
    if component.trim().is_empty() {
        bail!("a {CODE:?} auth's component must name a file beside this document");
    }
    if component.starts_with('~') {
        bail!(
            "component {component:?} is packed from a file beside this document, so it cannot be home-anchored"
        );
    }
    let reaches_out = component.starts_with('/')
        || component
            .split('/')
            .any(|segment| segment.is_empty() || segment == "..");
    if reaches_out || component.chars().any(char::is_control) {
        bail!(
            "component {component:?} must name one file beside this document, by a relative path with no empty or parent segment"
        );
    }
    Ok(())
}

/// §3.2.5's checkable half: a secret-shaped name earns its exception only by carrying a placeholder **this method** declares.
pub fn refuse_a_secret_shaped_file_carrying_no_declared_placeholder(
    method: &Method,
    path_files: &BTreeMap<String, BTreeMap<String, Vec<u8>>>,
) -> Result<()> {
    let declared: Vec<&str> = method
        .credentials
        .iter()
        .map(|credential| credential.placeholder.as_str())
        .collect();
    let inline = method
        .filesets
        .iter()
        .flat_map(|fileset| fileset.inline.iter().flatten())
        .map(|(name, content)| (name, std::borrow::Cow::Borrowed(content.as_str())));
    let packed = method
        .filesets
        .iter()
        .filter_map(|fileset| fileset.path.as_ref())
        .filter_map(|path| path_files.get(path))
        .flatten()
        .map(|(name, bytes)| (name, String::from_utf8_lossy(bytes)));
    for (name, content) in inline.chain(packed) {
        if name.split('/').any(crate::sandbox::looks_like_secret_name)
            && !declared
                .iter()
                .any(|placeholder| content.contains(placeholder))
        {
            bail!(
                "connector fileset file {name} is secret-shaped and carries no placeholder this method declares; a connector writes the placeholder and the real value stays on the host"
            );
        }
    }
    Ok(())
}

/// Every `path` fileset a connector declares, with the method that owns it.
pub fn path_filesets(connector: &ConnectorSpec) -> Vec<(&str, &str)> {
    connector
        .methods
        .iter()
        .flat_map(|method| {
            method
                .filesets
                .iter()
                .filter_map(move |fileset| Some((method.name.as_str(), fileset.path.as_deref()?)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(spec: &str) -> Vec<u8> {
        format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"connector","name":"some-provider","spec":{spec}}}"#
        )
        .into_bytes()
    }

    fn code_of(auth: &Auth) -> CodeAuth {
        auth.code().expect("a code auth").expect("it reads")
    }

    fn code_auth_of(block: &str) -> Auth {
        let extra: BTreeMap<String, serde_json::Value> =
            serde_json::from_str(block).expect("a code block");
        Auth {
            kind: CODE.into(),
            extra,
            ..Auth::default()
        }
    }

    const SERVES: &str = r#""serves":["api.some-provider.example"]"#;

    fn drawing_on(field: Option<&str>) -> Vec<u8> {
        let field = field.map_or(String::new(), |field| format!(r#","field":"{field}""#));
        with_methods(&format!(
            r#"[{{"name":"token","auth":{{"kind":"token"}},"credentials":[{{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"{field}}}]}}]"#
        ))
    }

    #[test]
    fn a_field_naming_the_one_value_its_auth_produces_is_what_the_credential_draws_on() {
        let parsed =
            parse(&drawing_on(Some("token"))).expect("field names the token auth's output");
        let method = &parsed.spec.methods[0];
        assert_eq!(
            input_of(method, &method.credentials[0]),
            Some("token".to_string())
        );
    }

    #[test]
    fn a_credential_naming_no_field_draws_on_its_auth_s_one_value() {
        // Otherwise the connect and the grant each pick their own key, and the credential reaches the guest unarmed.
        let parsed = parse(&drawing_on(None)).expect("a credential may name no field");
        let method = &parsed.spec.methods[0];
        assert_eq!(
            input_of(method, &method.credentials[0]),
            Some("token".to_string())
        );
    }

    #[test]
    fn a_field_naming_something_its_auth_does_not_produce_is_refused() {
        // §4.1: nothing would supply that value, so the credential would install, connect, grant, and still reach the guest unarmed.
        let err = parse(&drawing_on(Some("access_token"))).unwrap_err();
        let rendered = format!("{err:#}");
        assert!(rendered.contains("access_token"), "{rendered}");
        assert!(rendered.contains("unarmed"), "{rendered}");
    }

    fn code_method(auth: &str, credentials: &str) -> Vec<u8> {
        with_methods(&format!(
            r#"[{{"name":"sign-in","auth":{auth},"credentials":[{credentials}]}}]"#
        ))
    }

    const CODE_AUTH: &str = r#"{"kind":"code","component":"./sign-in.wasm","outputs":["access_token","refresh_token"]}"#;

    fn credential_drawing_on(field: Option<&str>) -> String {
        let field = field.map_or(String::new(), |field| format!(r#","field":"{field}""#));
        format!(r#"{{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"{field}}}"#)
    }

    #[test]
    fn a_code_method_produces_what_it_says_it_produces() {
        // §3.2.2: a component produces whatever it declares, so `outputs` is what a credential's `field` is validated against.
        let parsed = parse(&code_method(
            CODE_AUTH,
            &credential_drawing_on(Some("access_token")),
        ))
        .expect("a code method declaring its outputs parses");
        let method = &parsed.spec.methods[0];
        assert_eq!(
            input_of(method, &method.credentials[0]),
            Some("access_token".to_string())
        );
    }

    #[test]
    fn a_code_method_whose_credential_draws_on_a_field_it_does_not_produce_is_refused() {
        let err = parse(&code_method(
            CODE_AUTH,
            &credential_drawing_on(Some("id_token")),
        ))
        .unwrap_err();
        let rendered = format!("{err:#}");
        assert!(rendered.contains("id_token"), "{rendered}");
        assert!(rendered.contains("unarmed"), "{rendered}");
    }

    #[test]
    fn a_code_method_declaring_no_outputs_is_refused() {
        // Without `outputs`, `lns artifact validate` cannot check a code connector's credentials at all.
        let err = parse(&code_method(
            r#"{"kind":"code","component":"./sign-in.wasm"}"#,
            &credential_drawing_on(Some("access_token")),
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("outputs"), "{err:#}");
    }

    #[test]
    fn a_home_anchored_component_is_refused_for_the_reason_it_cannot_work() {
        // §3.2.6: the file is packed at publish, so it cannot name one on whichever machine runs the document.
        let err = parse(&code_method(
            r#"{"kind":"code","component":"~/sign-in.wasm","outputs":["token"]}"#,
            &credential_drawing_on(Some("token")),
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("packed"), "{err:#}");
    }

    #[test]
    fn a_component_reaching_outside_the_document_s_directory_is_refused() {
        let err = parse(&code_method(
            r#"{"kind":"code","component":"../elsewhere/sign-in.wasm","outputs":["token"]}"#,
            &credential_drawing_on(Some("token")),
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("beside"), "{err:#}");
    }

    #[test]
    fn an_absolute_component_is_refused() {
        let err = parse(&code_method(
            r#"{"kind":"code","component":"/opt/sign-in.wasm","outputs":["token"]}"#,
            &credential_drawing_on(Some("token")),
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("beside"), "{err:#}");
    }

    #[test]
    fn a_component_carrying_a_control_character_is_refused() {
        // A path a terminal renders as one thing and a filesystem opens as another is never worth resolving.
        let err = parse(&code_method(
            r#"{"kind":"code","component":"sign-in\u0007.wasm","outputs":["token"]}"#,
            &credential_drawing_on(Some("token")),
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("beside"), "{err:#}");
    }

    #[test]
    fn a_component_beside_the_document_may_be_written_with_a_leading_dot() {
        // §3.2.6's own example is `./sign-in.wasm`, so the ordinary spelling must parse.
        let parsed = parse(&code_method(
            r#"{"kind":"code","component":"./sign-in.wasm","outputs":["token"]}"#,
            &credential_drawing_on(Some("token")),
        ))
        .expect("the spec's own spelling parses");
        let auth = parsed.spec.methods[0].auth.as_ref().expect("auth");
        let code = auth.code().expect("a code auth").expect("it reads");
        assert_eq!(code.component.as_deref(), Some("./sign-in.wasm"));
    }

    #[test]
    fn a_component_that_is_only_whitespace_is_refused() {
        // It is not empty, so the field looks declared; nothing beside the document answers to it.
        let err = parse(&code_method(
            r#"{"kind":"code","component":"   ","outputs":["token"]}"#,
            &credential_drawing_on(Some("token")),
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("beside this document"),
            "{err:#}"
        );
    }

    #[test]
    fn an_empty_component_is_refused() {
        let err = parse(&code_method(
            r#"{"kind":"code","component":"","outputs":["token"]}"#,
            &credential_drawing_on(Some("token")),
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("component"), "{err:#}");
    }

    #[test]
    fn a_code_method_declaring_no_component_is_refused() {
        let err = parse(&code_method(
            r#"{"kind":"code","outputs":["access_token"]}"#,
            &credential_drawing_on(Some("access_token")),
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("component"), "{err:#}");
    }

    #[test]
    fn a_credential_naming_no_field_under_a_code_method_producing_several_is_refused() {
        // §4.1 lets a credential name no field only where the auth produces one value; several leaves nothing to say which.
        let err = parse(&code_method(CODE_AUTH, &credential_drawing_on(None))).unwrap_err();
        let rendered = format!("{err:#}");
        assert!(rendered.contains("access_token"), "{rendered}");
        assert!(rendered.contains("refresh_token"), "{rendered}");
    }

    #[test]
    fn a_code_method_producing_one_value_still_lets_a_credential_name_no_field() {
        let parsed = parse(&code_method(
            r#"{"kind":"code","component":"./sign-in.wasm","outputs":["token"]}"#,
            &credential_drawing_on(None),
        ))
        .expect("one output leaves nothing ambiguous");
        let method = &parsed.spec.methods[0];
        assert_eq!(
            input_of(method, &method.credentials[0]),
            Some("token".to_string())
        );
    }

    #[test]
    fn a_code_field_on_a_kind_that_is_not_code_is_refused() {
        // §5: the fields a component needs mean nothing to a mechanism lns implements itself.
        let err = parse(&code_method(
            r#"{"kind":"token","component":"./sign-in.wasm"}"#,
            &credential_drawing_on(None),
        ))
        .unwrap_err();
        let rendered = format!("{err:#}");
        assert!(rendered.contains("component"), "{rendered}");
        assert!(rendered.contains("code"), "{rendered}");
    }

    #[test]
    fn outputs_on_a_kind_that_is_not_code_is_refused() {
        // What a built-in produces is this version's to know, so a document restating it could disagree with the mechanism.
        let err = parse(&code_method(
            r#"{"kind":"token","outputs":["token"]}"#,
            &credential_drawing_on(None),
        ))
        .unwrap_err();
        let rendered = format!("{err:#}");
        assert!(rendered.contains("outputs"), "{rendered}");
        assert!(rendered.contains("code"), "{rendered}");
    }

    #[test]
    fn hosts_on_a_kind_that_is_not_code_is_refused() {
        // A built-in reaches what its mechanism reaches; a host list would read as a bound lns does not apply.
        let err = parse(&code_method(
            r#"{"kind":"token","hosts":["auth.some-provider.example"]}"#,
            &credential_drawing_on(None),
        ))
        .unwrap_err();
        let rendered = format!("{err:#}");
        assert!(rendered.contains("hosts"), "{rendered}");
        assert!(rendered.contains("code"), "{rendered}");
    }

    #[test]
    fn an_auth_producing_several_values_arms_no_credential_that_names_none() {
        // `input_of` is public and a caller may hold a method from somewhere the document check never ran, so it says "nothing" rather than guessing the first.
        let method = Method {
            name: "sign-in".into(),
            auth: Some(code_auth_of(
                r#"{"component":"./sign-in.wasm","outputs":["access_token","refresh_token"]}"#,
            )),
            ..Method::default()
        };
        let credential = lns_spec::Credential {
            env_var: Some("SOME_TOKEN".into()),
            placeholder: "some_LNSPLACEHOLDER0000000000".into(),
            field: None,
            injections: Vec::new(),
        };
        assert_eq!(input_of(&method, &credential), None);
    }

    #[test]
    fn a_code_method_declaring_one_output_twice_is_refused() {
        let err = parse(&code_method(
            r#"{"kind":"code","component":"./sign-in.wasm","outputs":["token","token"]}"#,
            &credential_drawing_on(Some("token")),
        ))
        .unwrap_err();
        let rendered = format!("{err:#}");
        assert!(rendered.contains("twice"), "{rendered}");
        assert!(rendered.contains("token"), "{rendered}");
    }

    #[test]
    fn a_code_method_is_not_offered_until_this_build_can_run_one() {
        // Offering it would ask the user to hand-type what a component is supposed to produce, and nothing here runs a component.
        let parsed = parse(&code_method(
            CODE_AUTH,
            &credential_drawing_on(Some("access_token")),
        ))
        .expect("a code method parses");
        assert!(
            !parsed.spec.methods[0].is_offerable(),
            "a code method must read as needing a newer lns while its mechanism is unimplemented"
        );
    }

    #[test]
    fn a_token_method_beside_an_unofferable_code_method_is_still_offered() {
        let document = with_methods(
            r#"[{"name":"sign-in","auth":{"kind":"code","component":"./sign-in.wasm","outputs":["token"]}},{"name":"paste","auth":{"kind":"token"}}]"#,
        );
        let parsed = parse(&document).expect("both methods parse");
        assert!(!parsed.spec.methods[0].is_offerable());
        assert!(parsed.spec.methods[1].is_offerable());
    }

    #[test]
    fn a_code_auth_is_still_decoded_strictly_though_it_is_not_offered() {
        // Unofferable is not unknown: this version knows the kind, so a typo in it is still a broken document.
        let err = parse(&code_method(
            r#"{"kind":"code","component":"./sign-in.wasm","outputs":["token"],"hostss":[]}"#,
            &credential_drawing_on(Some("token")),
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("hostss"), "{err:#}");
    }

    #[test]
    fn a_kind_this_version_does_not_know_may_carry_a_field_this_version_reserves() {
        // §3.2.2: an unknown kind decodes nothing, so a future kind that legitimately declares hosts must not make today's lns refuse the whole connector.
        let document = with_methods(
            r#"[{"name":"browser","auth":{"kind":"oauth_device","hosts":["auth.some-provider.example"]}},{"name":"token","auth":{"kind":"token"}}]"#,
        );
        let parsed =
            parse(&document).expect("an unknown kind carrying a reserved field still parses");
        assert!(!parsed.spec.methods[0].is_offerable());
        assert!(parsed.spec.methods[1].is_offerable());
    }

    #[test]
    fn a_kind_this_version_does_not_know_may_carry_a_shape_this_version_cannot_read() {
        // §3.2.2: an unknown kind decodes nothing, so a future kind's own spelling of a field must not refuse the document.
        for future in [
            r#""limits":{"callSeconds":30,"retries":3}"#,
            r#""exec":"yes""#,
            r#""outputs":"token""#,
            r#""component":{"path":"x"}"#,
        ] {
            let document = with_methods(&format!(
                r#"[{{"name":"browser","auth":{{"kind":"oauth_device",{future}}}}},{{"name":"token","auth":{{"kind":"token"}}}}]"#
            ));
            let parsed = parse(&document).expect("an unknown kind decodes nothing of its auth");
            assert!(!parsed.spec.methods[0].is_offerable(), "{future}");
            assert!(parsed.spec.methods[1].is_offerable(), "{future}");
        }
    }

    #[test]
    fn a_kind_this_version_does_not_know_may_carry_exec() {
        let document = with_methods(
            r#"[{"name":"browser","auth":{"kind":"oauth_device","exec":true}},{"name":"token","auth":{"kind":"token"}}]"#,
        );
        let parsed = parse(&document).expect("an unknown kind decodes nothing of its auth");
        assert!(!parsed.spec.methods[0].is_offerable());
    }

    #[test]
    fn the_specs_own_code_example_parses() {
        // §3.2.6's worked example, verbatim: a document the specification calls valid must not be refused.
        let parsed = parse(&code_method(
            r#"{"kind":"code","component":"./sign-in.wasm","outputs":["access_token"],"hosts":["auth.some-provider.example"],"limits":{"callSeconds":30,"sessionSeconds":900}}"#,
            &credential_drawing_on(Some("access_token")),
        ))
        .expect("the specification's own example");
        let auth = parsed.spec.methods[0].auth.as_ref().expect("auth");
        assert_eq!(code_of(auth).limits().call_seconds, 30);
        assert_eq!(code_of(auth).limits().session_seconds, 900);
    }

    #[test]
    fn a_call_deadline_past_the_ceiling_is_refused() {
        let err = parse(&code_method(
            r#"{"kind":"code","component":"./sign-in.wasm","outputs":["token"],"limits":{"callSeconds":31}}"#,
            &credential_drawing_on(Some("token")),
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("30"), "{err:#}");
    }

    #[test]
    fn a_session_deadline_past_the_ceiling_is_refused() {
        let err = parse(&code_method(
            r#"{"kind":"code","component":"./sign-in.wasm","outputs":["token"],"limits":{"sessionSeconds":901}}"#,
            &credential_drawing_on(Some("token")),
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("900"), "{err:#}");
    }

    #[test]
    fn limits_a_code_auth_leaves_out_default_to_the_ceiling() {
        let parsed = parse(&code_method(
            CODE_AUTH,
            &credential_drawing_on(Some("access_token")),
        ))
        .expect("limits is optional");
        let auth = parsed.spec.methods[0].auth.as_ref().expect("auth");
        assert_eq!(code_of(auth).limits().call_seconds, 30);
        assert_eq!(code_of(auth).limits().session_seconds, 900);
    }

    #[test]
    fn a_host_the_match_grammar_cannot_read_is_refused() {
        // The card names these and lns enforces them per call, so an unparseable entry is a bound that silently holds nothing.
        let err = parse(&code_method(
            r#"{"kind":"code","component":"./sign-in.wasm","outputs":["token"],"hosts":["not a host"]}"#,
            &credential_drawing_on(Some("token")),
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("not a host"), "{err:#}");
    }

    #[test]
    fn a_hosts_entry_naming_no_destination_is_refused() {
        // An empty entry reads as a bound and holds nothing, and the card would print a blank line as a disclosure.
        let err = parse(&code_method(
            r#"{"kind":"code","component":"./sign-in.wasm","outputs":["token"],"hosts":[""]}"#,
            &credential_drawing_on(Some("token")),
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must name a destination"),
            "{err:#}"
        );
    }

    #[test]
    fn a_hosts_entry_whose_port_position_is_not_a_port_is_refused() {
        // lns reads the whole entry as one host name, so the component could never reach what the author meant.
        let err = parse(&code_method(
            r#"{"kind":"code","component":"./sign-in.wasm","outputs":["token"],"hosts":["auth.some-provider.example:https"]}"#,
            &credential_drawing_on(Some("token")),
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("could never reach"), "{err:#}");
    }

    #[test]
    fn a_code_method_declares_whether_it_may_run_host_programs() {
        // §3.2.6: opt-in, because a method that does not ask earns the stronger card sentence.
        let parsed = parse(&code_method(
            r#"{"kind":"code","component":"./sign-in.wasm","outputs":["token"],"exec":true}"#,
            &credential_drawing_on(Some("token")),
        ))
        .expect("a code method may declare host execution");
        let auth = parsed.spec.methods[0].auth.as_ref().expect("auth");
        assert!(code_of(auth).exec);
    }

    #[test]
    fn a_code_method_runs_no_host_program_unless_it_says_so() {
        let parsed = parse(&code_method(
            CODE_AUTH,
            &credential_drawing_on(Some("access_token")),
        ))
        .expect("exec is optional");
        let auth = parsed.spec.methods[0].auth.as_ref().expect("auth");
        assert!(!code_of(auth).exec, "absent must mean no, never yes");
    }

    #[test]
    fn exec_on_a_kind_that_is_not_code_is_refused() {
        // A built-in mechanism runs no program of the document's choosing, so the field would read as a capability lns does not grant.
        let err = parse(&code_method(
            r#"{"kind":"token","exec":true}"#,
            &credential_drawing_on(None),
        ))
        .unwrap_err();
        let rendered = format!("{err:#}");
        assert!(rendered.contains("exec"), "{rendered}");
        assert!(rendered.contains("code"), "{rendered}");
    }

    #[test]
    fn a_code_method_declares_the_hosts_its_component_may_reach() {
        let parsed = parse(&code_method(
            r#"{"kind":"code","component":"./sign-in.wasm","outputs":["token"],"hosts":["auth.some-provider.example"]}"#,
            &credential_drawing_on(Some("token")),
        ))
        .expect("a code method may declare hosts");
        let auth = parsed.spec.methods[0].auth.as_ref().expect("auth");
        assert_eq!(
            code_of(auth).hosts,
            vec!["auth.some-provider.example".to_string()]
        );
    }

    #[test]
    fn a_field_under_a_kind_this_version_does_not_know_is_not_judged() {
        // §3.2.2: a reader that does not know a kind does not decode that auth at all, so what it produces is not this version's to check.
        let document = with_methods(
            r#"[{"name":"future","auth":{"kind":"oauth_device"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000","field":"access_token"}]}]"#,
        );
        let parsed = parse(&document).expect("an unknown kind parses");
        let method = &parsed.spec.methods[0];
        assert!(!method.is_offerable());
        assert_eq!(
            input_of(method, &method.credentials[0]),
            Some("access_token".to_string()),
            "the field it named is still what it would draw on, once a build knows the kind"
        );
    }

    #[test]
    fn a_credential_naming_no_field_under_an_unknown_kind_draws_on_nothing_this_version_can_name() {
        let document = with_methods(
            r#"[{"name":"future","auth":{"kind":"oauth_device"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}]}]"#,
        );
        let parsed = parse(&document).expect("an unknown kind parses");
        let method = &parsed.spec.methods[0];
        assert_eq!(input_of(method, &method.credentials[0]), None);
    }

    const CREDENTIAL: &str = r#"{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000","injections":[{"kind":"bearer_header","domain":"api.some-provider.example"}]}"#;

    fn with_methods(methods: &str) -> Vec<u8> {
        document(&format!(r#"{{{SERVES},"methods":{methods}}}"#))
    }

    #[test]
    fn a_method_carries_the_payload_a_grant_applies() {
        let def = parse(&with_methods(&format!(
            r#"[{{"name":"token","label":"API token","auth":{{"kind":"token","help":"where to get one"}},"egress":{{"http":[{{"match":"api.some-provider.example","verdict":"allow"}}]}},"credentials":[{CREDENTIAL}],"env":{{"SOME_PROVIDER_REGION":"eu"}},"filesets":[{{"guestPath":"~/.some-provider","inline":{{"config.json":"{{}}"}}}}]}}]"#
        )))
        .expect("§3.2.2 lets a method carry egress, credentials, env and a fileset");
        assert_eq!(def.name, "some-provider");
        assert_eq!(def.spec.serves, ["api.some-provider.example"]);
        let method = &def.spec.methods[0];
        assert_eq!(method.name, "token");
        assert_eq!(method.label(), "API token");
        assert_eq!(method.egress.http.len(), 1);
        assert_eq!(method.credentials[0].env_var.as_deref(), Some("SOME_TOKEN"));
        assert_eq!(
            method.env.get("SOME_PROVIDER_REGION").map(String::as_str),
            Some("eu")
        );
        assert_eq!(method.filesets[0].guest_path, "~/.some-provider");
        assert!(method.is_offerable());
    }

    #[test]
    fn a_method_with_no_auth_is_granted_rather_than_connected() {
        let def = parse(&document(
            r#"{"serves":["docs.rs"],"methods":[{"name":"default","egress":{"http":[{"match":"docs.rs","verdict":"allow"}]}}]}"#,
        ))
        .expect("a connector that only opens a destination has nothing to sign in to");
        let method = &def.spec.methods[0];
        assert!(method.auth.is_none());
        assert!(
            method.is_offerable(),
            "a method with nothing to authenticate is offerable, not lesser: the card grants it and never connects it"
        );
        assert_eq!(method.label(), "default", "label defaults to name");
    }

    #[test]
    fn a_method_refuses_each_block_a_running_guest_cannot_be_given() {
        for (block, payload) in [
            ("tools", r#"["postgresql@17"]"#),
            ("scripts", r#"[{"when":"pre-start","run":"echo hi"}]"#),
            (
                "volumes",
                r#"[{"name":"cache","target":"/home/agent/.cache"}]"#,
            ),
            ("ports", r#"[{"container":8080}]"#),
            ("mixins", r#"["./local"]"#),
            ("image", r#""ghcr.io/team/base:1""#),
            ("command", r#""agent --serve""#),
            ("workdir", r#""/workspace""#),
            ("user", r#""node""#),
            ("resources", r#"{"cpu":2}"#),
        ] {
            let err = parse(&with_methods(&format!(
                r#"[{{"name":"token","auth":{{"kind":"token"}},"{block}":{payload}}}]"#
            )))
            .unwrap_err();
            assert!(
                format!("{err:#}").contains(&format!("a connector must not declare {block}")),
                "§3.2.3 refuses a block by name so an author learns where it belongs; got: {err:#}"
            );
        }
    }

    #[test]
    fn a_payload_block_beside_the_methods_is_refused_where_it_belongs() {
        for block in METHOD_BLOCKS {
            let payload = match block {
                "egress" => r#"{"http":[]}"#.to_string(),
                "credentials" => format!("[{CREDENTIAL}]"),
                "filesets" => r#"[{"guestPath":"~/a","inline":{"a":"b"}}]"#.to_string(),
                _ => r#"{"A":"b"}"#.to_string(),
            };
            let err = parse(&document(&format!(
                r#"{{{SERVES},"{block}":{payload},"methods":[{{"name":"token","auth":{{"kind":"token"}}}}]}}"#
            )))
            .unwrap_err();
            assert!(
                format!("{err:#}")
                    .contains(&format!("a connector must declare {block} inside a method")),
                "a grant applies one method, so a payload block outside them belongs to nothing; got: {err:#}"
            );
        }
    }

    #[test]
    fn a_connector_that_serves_nothing_is_refused() {
        let err = parse(&document(
            r#"{"methods":[{"name":"token","auth":{"kind":"token"}}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("at least one serves entry"),
            "serves is what decides when a connector is offered, so one that serves nothing can never be offered; got: {err:#}"
        );
    }

    #[test]
    fn a_connector_with_no_method_is_refused() {
        let err = parse(&document(&format!(r#"{{{SERVES},"methods":[]}}"#))).unwrap_err();
        assert!(
            format!("{err:#}").contains("at least one method"),
            "methods are how a connector is connected; got: {err:#}"
        );
    }

    #[test]
    fn two_methods_sharing_a_name_are_refused() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"}},{"name":"token","auth":{"kind":"token"}}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate connector method"),
            "a grant records the method by name, so two methods answering to one name would make a grant ambiguous; got: {err:#}"
        );
    }

    #[test]
    fn a_method_name_that_is_not_a_dns_label_is_refused() {
        let err = parse(&with_methods(
            r#"[{"name":"Token Method","auth":{"kind":"token"}}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid method name"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_method_declaring_credentials_without_auth_is_refused() {
        let err = parse(&with_methods(&format!(
            r#"[{{"name":"token","credentials":[{CREDENTIAL}]}}]"#
        )))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must declare auth"),
            "nothing would produce the value, so the credential would ship permanently unarmed; got: {err:#}"
        );
    }

    #[test]
    fn an_unknown_auth_kind_parses_and_leaves_its_method_unofferable() {
        let def = parse(&with_methods(
            r#"[{"name":"browser","auth":{"kind":"oauth_device","clientId":"abc","scopes":["read"]}},{"name":"token","auth":{"kind":"token"}}]"#,
        ))
        .expect(
            "§3.2.2 makes an unknown kind a stated exception to strict decoding: refusing the document would make every improved connector uninstallable on a machine that had not upgraded",
        );
        assert!(
            !def.spec.methods[0].is_offerable(),
            "a mechanism this version cannot run must not be offered"
        );
        assert!(
            def.spec.methods[1].is_offerable(),
            "every other method in the document is still offered"
        );
    }

    #[test]
    fn a_typo_in_a_known_auth_kind_still_fails_the_load() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token","helpp":"typo"}}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("unknown field \"helpp\""),
            "the exception is bounded to an auth whose kind this version does not know; got: {err:#}"
        );
    }

    #[test]
    fn a_method_refuses_a_fileset_that_reads_the_machine_running_it() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"hostPath":"~/.some-provider/config.json","guestPath":"~/.some-provider/config.json"}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must not declare a fileset hostPath"),
            "a connector is installed once and used in every project, so reading a path off whichever machine runs it is a sandbox concern; got: {err:#}"
        );
    }

    #[test]
    fn a_connectors_dotted_credentials_file_still_has_to_carry_a_placeholder() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}],"filesets":[{"guestPath":"~/.claude","inline":{".credentials.json":"{\"accessToken\":\"sk-live-real\"}"}}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("carries no placeholder"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_connector_fileset_writes_under_the_guests_home() {
        let def = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"guestPath":"~/.some-provider","inline":{"config.json":"{}"}}]}]"#,
        ))
        .expect("§3.2.3 anchors a connector fileset at the home the running guest reports");
        assert_eq!(
            def.spec.methods[0].filesets[0].guest_path,
            "~/.some-provider"
        );
    }

    #[test]
    fn a_connector_fileset_may_not_name_an_absolute_guest_path() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"guestPath":"/home/agent/.some-provider","inline":{"config.json":"{}"}}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must start with `~/`"),
            "a method is applied on a policy change, which reaches only the home of the running guest, so an absolute path is refused where the document is parsed rather than at grant; got: {err:#}"
        );
    }

    #[test]
    fn a_connector_fileset_may_not_name_another_users_home() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"guestPath":"~alice/.some-provider","inline":{"config.json":"{}"}}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must start with `~/`"),
            "a document does not choose whose files it writes; got: {err:#}"
        );
    }

    #[test]
    fn a_methods_filesets_are_counted_together_against_one_ceiling() {
        let chunk = "a".repeat(120 * 1024);
        let files = (0..5)
            .map(|i| format!(r#""f{i}.txt":"{chunk}""#))
            .collect::<Vec<_>>()
            .join(",");
        let err = parse(&with_methods(&format!(
            r#"[{{"name":"token","auth":{{"kind":"token"}},"filesets":[{{"guestPath":"~/.a","inline":{{{files}}}}},{{"guestPath":"~/.b","inline":{{{files}}}}}]}}]"#
        )))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("this method's filesets total"),
            "each fileset clears the per-fileset limit on its own, so counting them apart would let one method write more than a method may; got: {err:#}"
        );
    }

    #[test]
    fn a_method_writing_exactly_the_ceiling_is_kept() {
        let chunk = "a".repeat(MAX_METHOD_FILESET_BYTES / 8);
        let files = (0..8)
            .map(|i| format!(r#""f{i}.txt":"{chunk}""#))
            .collect::<Vec<_>>()
            .join(",");
        parse(&with_methods(&format!(
            r#"[{{"name":"token","auth":{{"kind":"token"}},"filesets":[{{"guestPath":"~/.a","inline":{{{files}}}}}]}}]"#
        )))
        .expect("the rule is MUST NOT exceed, so the ceiling itself is a size a method may write");
    }

    #[test]
    fn a_method_may_not_set_a_variable_a_credential_of_its_own_fills() {
        // One variable holds one value, which the install already refuses between two connectors; inside one method nothing would decide which of the two the workload reads.
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"env":{"SOME_TOKEN":"plain"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("SOME_TOKEN"),
            "the refusal must name the variable claimed twice; got: {err:#}"
        );
    }

    #[test]
    fn two_filesets_in_one_method_may_not_claim_one_guest_path() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"guestPath":"~/.x","inline":{"a":"1"}},{"guestPath":"~/.x","inline":{"b":"2"}}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate guest path"),
            "the card names every fileset a method writes, so two entries claiming one path would disclose two writes where one happens; got: {err:#}"
        );
    }

    #[test]
    fn one_guest_path_written_two_ways_is_still_one_guest_path() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"guestPath":"~/.x","inline":{"a":"1"}},{"guestPath":"~/.x/","inline":{"b":"2"}}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate guest path"),
            "a trailing slash is a spelling, not a second directory, and comparing the raw strings would let a method claim one path twice; got: {err:#}"
        );
    }

    #[test]
    fn two_of_a_methods_files_may_not_land_on_one_path() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"guestPath":"~/.x","inline":{"b/c.json":"1"}},{"guestPath":"~/.x/b","inline":{"c.json":"2"}}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("land on ~/.x/b/c.json"),
            "the directories differ, so only the resolved file paths show that the guest is asked to hold one file twice; got: {err:#}"
        );
    }

    #[test]
    fn two_methods_may_each_write_the_same_guest_path() {
        parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"guestPath":"~/.x","inline":{"a":"1"}}]},{"name":"sso","auth":{"kind":"token"},"filesets":[{"guestPath":"~/.x","inline":{"b":"2"}}]}]"#,
        ))
        .expect("methods are alternatives, so only one ever writes and the paths cannot collide");
    }

    #[test]
    fn a_placeholder_that_could_pass_for_a_real_token_is_refused() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"sk-live-0123456789"}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must self-identify as fake"),
            "these documents are pushed to registries, so a placeholder that reads like a token is a credential one push from being public; got: {err:#}"
        );
    }

    #[test]
    fn a_credential_inside_a_method_may_carry_no_env_var() {
        let def = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"credentials":[{"placeholder":"some_LNSPLACEHOLDER0000000000","injections":[{"kind":"bearer_header","domain":"api.some-provider.example"}]}],"filesets":[{"guestPath":"~/.some-provider/credentials.json","inline":{"credentials.json":"{\"token\":\"some_LNSPLACEHOLDER0000000000\"}"}}]}]"#,
        ))
        .expect("a fileset-delivered credential serves a client that never reads the environment");
        let credential = &def.spec.methods[0].credentials[0];
        assert!(credential.env_var.is_none());
        assert_eq!(
            credential.owner(),
            "some_LNSPLACEHOLDER0000000000",
            "the placeholder is the entry's identity when no variable names it"
        );
    }

    #[test]
    fn two_credentials_in_one_method_sharing_a_placeholder_are_refused() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"credentials":[{"envVar":"A","placeholder":"some_LNSPLACEHOLDER0000000000"},{"envVar":"B","placeholder":"some_LNSPLACEHOLDER0000000000"}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate credential placeholder"),
            "the placeholder is what the merge keys on for an entry with no envVar; got: {err:#}"
        );
    }

    #[test]
    fn two_methods_may_reuse_one_variable_and_one_marker() {
        parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}]},{"name":"sso","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}]}]"#,
        ))
        .expect(
            "methods are alternatives and only one ever enters the merge, so a token method and a sign-in method serve the same variable of the same service",
        );
    }

    #[test]
    fn two_methods_serving_one_variable_with_two_markers_are_refused() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}]},{"name":"sso","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"other_LNSPLACEHOLDER000000000"}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("two placeholders"),
            "a run that moves between these methods would leave its live sessions holding the other method's marker; got: {err:#}"
        );
    }

    #[test]
    fn a_secret_shaped_inline_file_must_carry_a_placeholder_its_own_method_declares() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}],"filesets":[{"guestPath":"~/.some-provider","inline":{"credentials.json":"{\"token\":\"sk-live-real\"}"}}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("carries no placeholder this method declares"),
            "§3.2.5's checkable half: a secret-shaped name earns its exception only by carrying the marker; got: {err:#}"
        );
    }

    #[test]
    fn a_secret_shaped_file_cannot_borrow_a_sibling_methods_marker() {
        let err = parse(&with_methods(
            r#"[{"name":"a","auth":{"kind":"token"},"credentials":[{"envVar":"A_TOKEN","placeholder":"a_LNSPLACEHOLDER00000000"}]},{"name":"b","auth":{"kind":"token"},"credentials":[{"envVar":"B_TOKEN","placeholder":"b_LNSPLACEHOLDER00000000"}],"filesets":[{"guestPath":"~/.p","inline":{"credentials.json":"{\"token\":\"a_LNSPLACEHOLDER00000000\"}"}}]}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("carries no placeholder this method declares"),
            "a sibling's injection is not armed when this method applies, so its marker would reach the wire unsubstituted; got: {err:#}"
        );
    }

    #[test]
    fn a_path_fileset_is_checked_against_the_bytes_beside_the_document() {
        let json = with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}],"filesets":[{"path":"./some-provider","guestPath":"~/.some-provider"}]}]"#,
        );
        let beside = BTreeMap::from([(
            "./some-provider".to_string(),
            BTreeMap::from([(
                "credentials.json".to_string(),
                br#"{"token":"sk-live-real"}"#.to_vec(),
            )]),
        )]);
        let err = parse_with_path_files(&json, &beside).unwrap_err();
        assert!(
            format!("{err:#}").contains("carries no placeholder this method declares"),
            "got: {err:#}"
        );
        assert!(
            parse(&json).is_ok(),
            "a caller that cannot read the directory leaves the content check to the one that can"
        );
    }

    #[test]
    fn path_filesets_names_the_method_that_packs_each_directory() {
        let def = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"filesets":[{"path":"./a","guestPath":"~/a"}]},{"name":"sso","auth":{"kind":"token"},"filesets":[{"path":"./b","guestPath":"~/b"}]}]"#,
        ))
        .expect("two methods may each pack a directory");
        assert_eq!(
            path_filesets(&def.spec),
            [("token", "./a"), ("sso", "./b")],
            "an artifact carries one layer per entry, in declaration order"
        );
    }

    #[test]
    fn a_serves_entry_that_names_no_destination_is_refused() {
        for serves in [r#"[""]"#, r#"["  "]"#, r#"["api.example .com"]"#] {
            let err = parse(&document(&format!(
                r#"{{"serves":{serves},"methods":[{{"name":"token","auth":{{"kind":"token"}}}}]}}"#
            )))
            .unwrap_err();
            assert!(
                format!("{err:#}").contains("serves entry"),
                "{serves}: got {err:#}"
            );
        }
    }

    #[test]
    fn a_serves_entry_whose_port_position_is_not_a_port_is_refused() {
        // A tail that is not a port is read as part of one long host name, so the connector installs, lists, and is never offered — with no error at any point.
        for serves in [
            r#"["db.internal:notaport"]"#,
            r#"["api.example:99999"]"#,
            r#"["db.internal:"]"#,
            r#"["[::1]:notaport"]"#,
        ] {
            let err = parse(&document(&format!(
                r#"{{"serves":{serves},"methods":[{{"name":"token","auth":{{"kind":"token"}}}}]}}"#
            )))
            .unwrap_err();
            assert!(
                format!("{err:#}").contains("is not a port number"),
                "{serves}: got {err:#}"
            );
        }
    }

    #[test]
    fn a_serves_entry_that_names_a_port_or_none_at_all_is_kept() {
        for serves in [
            r#"["api.example"]"#,
            r#"["api.example:443"]"#,
            r#"["*.example:5432"]"#,
            r#"["[2001:db8::1]:443"]"#,
            r#"["2001:db8::1"]"#,
        ] {
            parse(&document(&format!(
                r#"{{"serves":{serves},"methods":[{{"name":"token","auth":{{"kind":"token"}}}}]}}"#
            )))
            .unwrap_or_else(|e| panic!("{serves} names a destination this rule must keep: {e:#}"));
        }
    }

    #[test]
    fn an_unknown_field_in_a_connector_spec_fails_the_load() {
        let err = parse(&document(&format!(
            r#"{{{SERVES},"methods":[{{"name":"token","auth":{{"kind":"token"}}}}],"servs":["typo.example"]}}"#
        )))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("unknown field \"servs\""),
            "a misspelled key must fail on its line rather than load with a default; got: {err:#}"
        );
    }

    #[test]
    fn an_unknown_field_in_a_method_fails_the_load() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"labell":"typo"}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("unknown field \"labell\""),
            "got: {err:#}"
        );
    }

    #[test]
    fn an_auth_that_names_no_kind_is_refused() {
        let err = parse(&with_methods(r#"[{"name":"token","auth":{"kind":"  "}}]"#)).unwrap_err();
        assert!(
            format!("{err:#}").contains("must name its kind"),
            "kind decides what else auth accepts and what the method produces; got: {err:#}"
        );
    }

    #[test]
    fn a_refused_block_beside_the_methods_is_still_refused_by_name() {
        let err = parse(&document(&format!(
            r#"{{{SERVES},"tools":["postgresql@17"],"methods":[{{"name":"token","auth":{{"kind":"token"}}}}]}}"#
        )))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("a connector must not declare tools"),
            "a block a running guest cannot take is refused wherever it sits, not only inside a method; got: {err:#}"
        );
    }

    #[test]
    fn a_refused_block_is_refused_however_it_is_written() {
        for payload in ["8080", "[]", "null"] {
            let err = parse(&with_methods(&format!(
                r#"[{{"name":"token","auth":{{"kind":"token"}},"ports":{payload}}}]"#
            )))
            .unwrap_err();
            assert!(
                format!("{err:#}").contains("a connector must not declare ports"),
                "writing the block at all is what a method may not do, so an empty or null one earns the same named refusal rather than a bare unknown-field error; got: {err:#}"
            );
        }
    }

    #[test]
    fn an_env_key_no_process_could_carry_is_refused() {
        let err = parse(&with_methods(
            r#"[{"name":"token","auth":{"kind":"token"},"env":{"SOME KEY":"v"}}]"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid env key"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_document_of_another_kind_is_refused() {
        let err = parse(
            br#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"x","spec":{"serves":["a.example"]}}"#,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("expected kind connector"),
            "got: {err:#}"
        );
    }
}
