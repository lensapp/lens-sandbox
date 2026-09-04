use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::matching::{port_shaped, split_destination};

pub mod decision_store;
pub mod host_bind_decisions;
pub mod host_path_decisions;
pub mod matching;
pub mod registry_auth;
pub mod secure_file;
#[cfg(test)]
mod test_env;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    #[serde(default)]
    pub network: NetworkPolicy,
    /// What the document on disk called itself, kept so writing a decision back does not rename the developer's own file.
    #[serde(skip)]
    pub name: Option<String>,
    /// Every block of that document the run does not write, kept verbatim so an approval cannot delete what somebody wrote by hand.
    #[serde(skip)]
    pub rest: serde_yaml::Mapping,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkPolicy {
    #[serde(default)]
    pub egress: Egress,
}

/// The document format every artifact is written in (`docs/sandbox-spec.md` §2), which a run's own decisions are written in too.
pub const API_VERSION: &str = "lns.run/v1";

/// The kind §8.1 records a decision as, so it merges under the same rules as anything the developer pulled.
pub const KIND: &str = "mixin";

/// What §4.2's `description` says about the one entry nobody typed, so a destination the run wrote down does not read as one somebody chose on purpose.
const APPROVED_NOTE: &str = "approved during a run";

/// The decisions file on disk. Its envelope is restated here rather than read through the crate that parses an artifact, which depends on this one; only the block a run writes back is modelled, and every other one travels in [`LocalMixinSpec::rest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LocalMixinDocument {
    api_version: String,
    kind: String,
    name: String,
    #[serde(default)]
    spec: LocalMixinSpec,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LocalMixinSpec {
    #[serde(default)]
    egress: Egress,
    #[serde(flatten)]
    rest: serde_yaml::Mapping,
}

/// The shape §2 requires of a document's name, restated here for the same reason the envelope is: a document this crate accepts has to be one the resolver accepts.
fn is_dns_label(name: &str) -> bool {
    let bytes = name.as_bytes();
    let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    !bytes.is_empty()
        && bytes.len() <= 63
        && alnum(bytes[0])
        && alnum(bytes[bytes.len() - 1])
        && bytes.iter().all(|&b| alnum(b) || b == b'-')
}

/// Name a written document after the file it lands in, since nobody is present to choose one.
fn dns_label_for(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let spelled: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let label: String = spelled.trim_matches('-').chars().take(63).collect();
    let label = label.trim_end_matches('-');
    if is_dns_label(label) {
        label.to_string()
    } else {
        "local".to_string()
    }
}

/// The per-protocol egress tables lens-sandbox-core routes on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "EgressRaw")]
pub struct Egress {
    pub http: Vec<RouteRule>,
    /// Raw-TCP pre-filter: a destination a rule here claims is spliced opaquely and never reaches [`Self::http`]; one no rule here matches falls through to it.
    pub tcp: Vec<TcpEgressRule>,
}

/// Deserialization shim: an `egress.tcp` rule core would refuse fails the load here rather than force-denying the whole policy inside the guest.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EgressRaw {
    #[serde(default)]
    http: Vec<RouteRule>,
    #[serde(default)]
    tcp: Vec<TcpEgressRule>,
}

impl TryFrom<EgressRaw> for Egress {
    type Error = String;

    fn try_from(raw: EgressRaw) -> Result<Self, Self::Error> {
        for rule in &raw.tcp {
            rule.validate()?;
        }
        Ok(Self {
            http: raw.http,
            tcp: raw.tcp,
        })
    }
}

impl Egress {
    /// Whether the first catch-all `http` rule the gate reaches is a deny, which decides everything the named rules leave rather than asking.
    pub fn is_closed(&self) -> bool {
        self.http
            .iter()
            .find(|rule| rule.is_catch_all())
            .is_some_and(|rule| rule.verdict == Verdict::Deny)
    }

    pub fn validate_local_transport(&self) -> io::Result<()> {
        let uses_upstream = self
            .http
            .iter()
            .any(|route| route.transport == Transport::Upstream);
        if uses_upstream {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "upstream transport isn't supported in the local sandbox",
            ));
        }
        Ok(())
    }

    pub fn validate_binary_scopes(&self) -> io::Result<()> {
        self.http.iter().try_for_each(RouteRule::validate_binaries)
    }
}

impl NetworkPolicy {
    pub fn is_closed(&self) -> bool {
        self.egress.is_closed()
    }

    pub fn validate_local_transport(&self) -> io::Result<()> {
        self.egress.validate_local_transport()
    }

    pub fn validate_binary_scopes(&self) -> io::Result<()> {
        self.egress.validate_binary_scopes()
    }
}

pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteRule {
    // The schema field is named `match`; rename keeps the Rust ident legal.
    #[serde(rename = "match")]
    pub match_pattern: String,
    pub verdict: Verdict,
    #[serde(default)]
    pub transport: Transport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<Scheme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub tls_terminate: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<HttpRule>,
    /// Absolute guest binary paths this rule is scoped to; absent means any caller, and a listed rule denies every other caller rather than falling through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binaries: Option<Vec<String>>,
}

/// One raw-TCP egress rule: a port-scoped destination the guest splices through untouched — no TLS interception and no HTTP rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TcpEgressRule {
    #[serde(rename = "match")]
    pub match_pattern: String,
    pub verdict: Verdict,
    /// Absolute `/proc/<pid>/exe` paths the rule is scoped to; omitted means any caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binaries: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl TcpEgressRule {
    pub fn allow_destination(pattern: impl Into<String>) -> Self {
        Self::new(pattern, Verdict::Allow)
    }

    pub fn deny_destination(pattern: impl Into<String>) -> Self {
        Self::new(pattern, Verdict::Deny)
    }

    pub fn new(pattern: impl Into<String>, verdict: Verdict) -> Self {
        Self {
            match_pattern: pattern.into(),
            verdict,
            binaries: None,
            description: None,
        }
    }

    pub fn approved(mut self) -> Self {
        self.description = Some(APPROVED_NOTE.to_string());
        self
    }

    /// Rejects everything lens-sandbox-core's `parse_tcp_egress` rejects — one rule it refuses force-denies the whole policy inside the guest — plus the scopes that parse there but can never match a caller.
    pub fn validate(&self) -> Result<(), String> {
        if destination_port(&self.match_pattern)? == 0 {
            return Err(format!(
                "egress.tcp rule {:?}: port 0 is not a valid destination port",
                self.match_pattern
            ));
        }
        let Some(binaries) = &self.binaries else {
            return Ok(());
        };
        if binaries.is_empty() {
            return Err("binaries filter is empty: it matches no caller and would deny the host for everyone; omit `binaries` to allow any caller".to_string());
        }
        for path in binaries {
            if let Some(why) = unmatchable_binary(path) {
                return Err(format!(
                    "binaries entry {path:?} {why}; entries are matched against the kernel-resolved /proc/<pid>/exe, so it can never match"
                ));
            }
        }
        Ok(())
    }
}

/// Why a binaries entry can never name a caller, or `None` when it can, judged the way the guest kernel will judge it.
fn unmatchable_binary(binary: &str) -> Option<&'static str> {
    let path = Path::new(binary);
    if !binary.starts_with('/') {
        return Some("is not an absolute path");
    }
    if path.components().any(|c| c == Component::ParentDir) {
        return Some("climbs through a \"..\" segment");
    }
    if path.file_name().is_none() {
        return Some("names no binary");
    }
    None
}

/// The destination port an `egress.tcp` `match` names, mirroring lens-sandbox-core's `parse_matcher`; a portless matcher is an error because a raw splice is granted with no inspection at all.
fn destination_port(pattern: &str) -> Result<u16, String> {
    if parses_as_cidr(pattern) {
        return Err(needs_a_port(pattern));
    }
    if pattern.starts_with('[') {
        return bracketed_port(pattern);
    }
    if pattern.matches(':').count() > 1 {
        return Err(format!(
            "ambiguous IPv6 address without brackets: {pattern}; use [addr]:port notation"
        ));
    }
    let (host, Some(port)) = split_destination(pattern) else {
        return Err(needs_a_port(pattern));
    };
    prefixed_host_is_a_range(pattern, host)?;
    named_host(pattern, host)?;
    parse_port(pattern, port)
}

fn bracketed_port(pattern: &str) -> Result<u16, String> {
    let (host, port) = split_destination(pattern);
    let Some(port) = port else {
        return Err(format!("invalid bracketed address: {pattern}"));
    };
    prefixed_host_is_a_range(pattern, host)?;
    named_host(pattern, host)?;
    // A `]:` puts the tail where only a port belongs, so one that isn't a port is broken rather than missing.
    if !port_shaped(port) {
        return Err(format!("invalid port in {pattern}"));
    }
    parse_port(pattern, port)
}

/// A prefix core's CIDR parser rejects reads as a hostname there, which resolves to nothing, so a `/` that is not a range is refused rather than written as a rule that never fires.
fn prefixed_host_is_a_range(pattern: &str, host: &str) -> Result<(), String> {
    if host.contains('/') && !parses_as_cidr(host) {
        return Err(format!("invalid CIDR in {pattern}"));
    }
    Ok(())
}

fn named_host(pattern: &str, host: &str) -> Result<(), String> {
    if host.is_empty() {
        return Err(format!(
            "egress.tcp rule {pattern:?} names no host before its port"
        ));
    }
    Ok(())
}

/// The port a pattern's numeric tail names; one too large gets its own error rather than being sent looking for a missing colon.
fn parse_port(pattern: &str, port: &str) -> Result<u16, String> {
    port.parse::<u16>().map_err(|_| {
        format!("egress.tcp rule {pattern:?}: {port} is not a valid port number (1-65535)")
    })
}

fn parses_as_cidr(pattern: &str) -> bool {
    pattern.parse::<ipnet::IpNet>().is_ok()
}

fn needs_a_port(pattern: &str) -> String {
    format!(
        "egress.tcp rule {pattern:?} must specify a port, e.g. \"host:443\" or \"10.0.0.0/24:443\""
    )
}

/// HTTP method/path restriction within a route rule; wire-compatible with lens-sandbox-core's `HttpRule`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// What a rule decides; there is no `ask`, because being asked is the absence of a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Allow,
    Deny,
}

impl<'de> Deserialize<'de> for Verdict {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "allow" => Ok(Self::Allow),
            "deny" => Ok(Self::Deny),
            "ask" => Err(serde::de::Error::custom(
                "verdict `ask` is not written in a policy file: a destination no rule decides is asked about already, so delete the rule and the approval card will appear on first use",
            )),
            other => Err(serde::de::Error::custom(format!(
                "unknown verdict {other:?}: expected `allow` or `deny`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Upstream,
    #[default]
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    Http,
    Https,
}

impl Policy {
    pub fn load_or_default(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            // §8.1 has the mixin exist whether or not anyone wrote it, and an editor truncates a file before it writes one.
            Ok(text) if text.trim().is_empty() => Ok(Self::default()),
            Ok(text) => Self::from_document(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    fn from_document(text: &str) -> io::Result<Self> {
        let invalid = |message: String| io::Error::new(io::ErrorKind::InvalidData, message);
        let doc: LocalMixinDocument =
            serde_yaml::from_str(text).map_err(|e| invalid(e.to_string()))?;
        if doc.api_version != API_VERSION {
            return Err(invalid(format!(
                "this document says apiVersion {}, and a directory's decisions are written in {API_VERSION}",
                doc.api_version
            )));
        }
        if doc.kind != KIND {
            return Err(invalid(format!(
                "this document says kind {}, and a directory's decisions are a {KIND}",
                doc.kind
            )));
        }
        if !is_dns_label(&doc.name) {
            return Err(invalid(format!(
                "this document is named {}, and a document is named with lowercase letters, digits and dashes, starting and ending with a letter or a digit",
                doc.name
            )));
        }
        let policy = Self {
            network: NetworkPolicy {
                egress: doc.spec.egress,
            },
            name: Some(doc.name),
            rest: doc.spec.rest,
        };
        policy.network.validate_local_transport()?;
        policy.network.validate_binary_scopes()?;
        Ok(policy)
    }

    /// The `mixin` document this policy is, named for the file it will be read from when it carries no name of its own.
    pub fn document_bytes(&self, path: &Path) -> io::Result<Vec<u8>> {
        let document = LocalMixinDocument {
            api_version: API_VERSION.to_string(),
            kind: KIND.to_string(),
            name: self.name.clone().unwrap_or_else(|| dns_label_for(path)),
            spec: LocalMixinSpec {
                egress: self.network.egress.clone(),
                rest: self.rest.clone(),
            },
        };
        serde_yaml::to_string(&document)
            .map(String::into_bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub fn save_atomic(&self, path: &Path) -> io::Result<()> {
        crate::secure_file::write_yaml_document_atomic(path, &self.document_bytes(path)?)
    }

    pub fn add_rule(&mut self, rule: RouteRule) {
        if !self.network.egress.http.contains(&rule) {
            self.network.egress.http.push(rule);
        }
    }

    /// Records a decision the developer just made on a held request; see [`place_approved`].
    pub fn add_approved_rule(&mut self, rule: RouteRule) -> Approval {
        let shadowing = self
            .network
            .first_shadowing_rule(&rule)
            .map(|(index, shadowing)| (index, shadowing.clone()));
        place_approved(&mut self.network.egress.http, rule, shadowing)
    }

    /// Records a decision the developer just made on a held raw-TCP request; see [`place_approved`].
    pub fn add_approved_tcp_rule(&mut self, rule: TcpEgressRule) -> Approval {
        let shadowing = self
            .network
            .first_shadowing_tcp_rule(&rule)
            .map(|(index, shadowing)| (index, shadowing.clone()));
        place_approved(&mut self.network.egress.tcp, rule, shadowing)
    }

    /// Takes back the entry an approval wrote for this destination; see [`remove_approved`].
    pub fn remove_approved_rule(&mut self, pattern: &str) -> bool {
        remove_approved(&mut self.network.egress.http, pattern)
    }

    /// Takes back the raw entry an approval wrote for this destination; see [`remove_approved`].
    pub fn remove_approved_tcp_rule(&mut self, destination: &str) -> bool {
        remove_approved(&mut self.network.egress.tcp, destination)
    }
}

/// What became of a decision meant to outlive the request it was made on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Approval {
    /// The decision is in force from here on, either as a rule just written or as one the file already held.
    Stands,
    /// Nothing was written: the named rule already decides every request this one could match, and the gate stops at the first match.
    Shadowed(String),
    /// Nothing was written: the table already holds this exact rule, but behind the named rule the gate reaches first, so reordering it is the author's to make.
    Unreachable(String),
}

/// A rule table entry the gate scans first-match-wins.
trait Placed: PartialEq {
    fn verdict(&self) -> Verdict;
    fn match_pattern(&self) -> &str;
    /// Whether an approval wrote this entry, which is the only kind an answer may take back.
    fn is_approved(&self) -> bool;
    /// Whether the file already holds this entry, disregarding the note beside it: a note says how an entry got there, never what it decides.
    fn already_held_as(&self, other: &Self) -> bool;
}

impl Placed for RouteRule {
    fn verdict(&self) -> Verdict {
        self.verdict
    }

    fn match_pattern(&self) -> &str {
        &self.match_pattern
    }

    fn is_approved(&self) -> bool {
        self.description.as_deref() == Some(APPROVED_NOTE)
    }

    fn already_held_as(&self, other: &Self) -> bool {
        let unnoted = |rule: &Self| Self {
            description: None,
            ..rule.clone()
        };
        unnoted(self) == unnoted(other)
    }
}

impl Placed for TcpEgressRule {
    fn verdict(&self) -> Verdict {
        self.verdict
    }

    fn match_pattern(&self) -> &str {
        &self.match_pattern
    }

    fn is_approved(&self) -> bool {
        self.description.as_deref() == Some(APPROVED_NOTE)
    }

    fn already_held_as(&self, other: &Self) -> bool {
        let unnoted = |rule: &Self| Self {
            description: None,
            ..rule.clone()
        };
        unnoted(self) == unnoted(other)
    }
}

/// Places an approval-derived rule where the gate reaches it: appended when nothing covers the destination, and otherwise written neither ahead of the rule that already decides it — that would grant more than the card showed — nor behind it, where it would never fire.
fn place_approved<R: Placed>(
    table: &mut Vec<R>,
    rule: R,
    shadowing: Option<(usize, R)>,
) -> Approval {
    let held = table
        .iter()
        .position(|existing| existing.already_held_as(&rule));
    let Some((index, shadowing)) = shadowing else {
        if held.is_none() {
            table.push(rule);
        }
        return Approval::Stands;
    };
    if let Some(held) = held {
        // A copy at the shadowing index is that rule itself, and one ahead of it is reached first; only a copy behind it is a line the gate never gets to.
        return if held <= index {
            Approval::Stands
        } else {
            Approval::Unreachable(shadowing.match_pattern().to_string())
        };
    }
    if shadowing.verdict() == rule.verdict() {
        return Approval::Stands;
    }
    Approval::Shadowed(shadowing.match_pattern().to_string())
}

/// Removes every entry an approval wrote for this destination, and only those: a rule the author typed is theirs to delete, even where it says the same thing.
fn remove_approved<R: Placed>(table: &mut Vec<R>, pattern: &str) -> bool {
    let before = table.len();
    table.retain(|rule| !(rule.is_approved() && rule.match_pattern() == pattern));
    table.len() != before
}

pub trait PolicyStore: Send + Sync {
    fn save(&self, policy: &Policy) -> io::Result<()>;
}

pub struct FilePolicyStore {
    pub path: PathBuf,
}

impl FilePolicyStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl PolicyStore for FilePolicyStore {
    fn save(&self, policy: &Policy) -> io::Result<()> {
        policy.save_atomic(&self.path)
    }
}

impl RouteRule {
    /// Whether this rule decides every destination for every caller, which makes it the file's backstop rather than a decision about anything in particular.
    pub fn is_catch_all(&self) -> bool {
        self.match_pattern == "*"
            && self.binaries.is_none()
            && self.rules.is_empty()
            && self.scheme.is_none()
    }

    pub fn allow_host(host: impl Into<String>) -> Self {
        Self {
            match_pattern: host.into(),
            verdict: Verdict::Allow,
            transport: Transport::Direct,
            scheme: None,
            description: None,
            tls_terminate: false,
            rules: Vec::new(),
            binaries: None,
        }
    }

    pub fn deny_host(host: impl Into<String>) -> Self {
        Self {
            match_pattern: host.into(),
            verdict: Verdict::Deny,
            transport: Transport::Direct,
            scheme: None,
            description: None,
            tls_terminate: false,
            rules: Vec::new(),
            binaries: None,
        }
    }

    pub fn approved(mut self) -> Self {
        self.description = Some(APPROVED_NOTE.to_string());
        self
    }

    pub fn validate_binaries(&self) -> io::Result<()> {
        let Some(binaries) = self.binaries.as_deref() else {
            return Ok(());
        };
        if binaries.is_empty() {
            return Err(invalid_data(format!(
                "the rule for {:?} has an empty binaries filter: it matches no caller, so it denies the host for everyone — omit binaries to let any caller through",
                self.match_pattern
            )));
        }
        binaries
            .iter()
            .try_for_each(|path| self.validate_binary(path))
    }

    fn validate_binary(&self, binary: &str) -> io::Result<()> {
        match unmatchable_binary(binary) {
            None => Ok(()),
            Some(why) => Err(invalid_data(format!(
                "the rule for {:?} lists the binary {binary:?}, which {why}: binaries are matched against the kernel-resolved /proc/<pid>/exe, so it can never match",
                self.match_pattern
            ))),
        }
    }
}

fn invalid_data(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Write a decisions file whose spec holds the given blocks, so a fixture states the one thing it is about.
    fn decisions_file(dir: &TempDir, spec: &str) -> PathBuf {
        let path = dir.path().join("decisions.yaml");
        let body: String = spec.lines().map(|line| format!("  {line}\n")).collect();
        fs::write(
            &path,
            format!("apiVersion: {API_VERSION}\nkind: {KIND}\nname: decisions\nspec:\n{body}"),
        )
        .unwrap();
        path
    }

    #[test]
    fn a_default_policy_decides_nothing_and_so_holds_no_rules() {
        // The file is the decisions the developer has made. A fresh one has made
        // none, and what happens to everything else is not a field any more.
        let p = Policy::default();
        assert!(p.network.egress.http.is_empty());
        assert!(p.network.egress.tcp.is_empty());
    }

    #[test]
    fn a_verdict_that_is_neither_allow_nor_deny_is_refused_naming_both() {
        // A typo must not read as one of the two, and the message has to say what the
        // author can write instead — there are only two answers now.
        let err = serde_yaml::from_str::<NetworkPolicy>(
            "egress:\n  http:\n    - match: api.example.test\n      verdict: bananas\n",
        )
        .expect_err("an unknown verdict must be refused");
        let msg = err.to_string();
        assert!(
            msg.contains("bananas") && msg.contains("`allow`") && msg.contains("`deny`"),
            "got {msg}"
        );
    }

    #[test]
    fn a_rule_asking_to_be_asked_is_refused_because_that_is_now_the_absence_of_a_rule() {
        // `ask` was how you said "prompt me about this". Nothing is decided
        // without a rule now, so the way to be asked is to write no rule — and a
        // file still saying it is telling the loader something it cannot honor.
        for table in ["http", "tcp"] {
            let pattern = if table == "tcp" {
                "db.internal:5432"
            } else {
                "api.example.test"
            };
            let err = serde_yaml::from_str::<NetworkPolicy>(&format!(
                "egress:\n  {table}:\n    - match: {pattern}\n      verdict: ask\n"
            ))
            .expect_err("an ask rule must be refused");
            let msg = err.to_string();
            assert!(
                msg.contains("ask"),
                "the {table} error must name the verdict; got {msg}"
            );
        }
    }

    #[test]
    fn default_policy_yaml_roundtrip_is_lossless() {
        let p = Policy::default();
        let yaml = serde_yaml::to_string(&p).unwrap();
        let parsed: Policy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(p, parsed);
    }

    #[test]
    fn policy_with_rules_yaml_roundtrip_is_lossless() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("api.linear.app"));
        p.add_rule(RouteRule::deny_host("evil.example"));

        let yaml = serde_yaml::to_string(&p).unwrap();
        let parsed: Policy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(p, parsed);
    }

    #[test]
    fn route_rule_serializes_with_match_key_not_match_pattern() {
        let rule = RouteRule::allow_host("api.linear.app");
        let yaml = serde_yaml::to_string(&rule).unwrap();
        assert!(
            yaml.contains("match:"),
            "expected `match:` key in YAML, got:\n{yaml}"
        );
        assert!(
            !yaml.contains("matchPattern"),
            "rust field name leaked into wire format:\n{yaml}"
        );
    }

    #[test]
    fn schemes_serialize_in_lowercase() {
        assert_eq!(serde_yaml::to_string(&Scheme::Http).unwrap().trim(), "http");
        assert_eq!(
            serde_yaml::to_string(&Scheme::Https).unwrap().trim(),
            "https"
        );
    }

    #[test]
    fn a_plain_route_omits_the_richness_keys_but_keeps_the_required_transport() {
        let yaml = serde_yaml::to_string(&RouteRule::allow_host("api.example.com")).unwrap();
        assert!(
            yaml.contains("transport: direct"),
            "the sandbox-core route schema has no transport default; a rule without one fails the whole route parse and forces deny:\n{yaml}"
        );
        assert!(
            !yaml.contains("scheme") && !yaml.contains("tlsTerminate") && !yaml.contains("rules"),
            "a plain allow rule must stay minimal:\n{yaml}"
        );
    }

    #[test]
    fn a_rich_route_rule_round_trips_with_sandbox_core_wire_names() {
        let rule = RouteRule {
            match_pattern: "gitlab.com".into(),
            verdict: Verdict::Allow,
            transport: Transport::Direct,
            scheme: Some(Scheme::Https),
            description: None,
            tls_terminate: true,
            rules: vec![HttpRule {
                method: Some("GET".into()),
                path: Some("/api/v4/**".into()),
            }],
            binaries: None,
        };
        let yaml = serde_yaml::to_string(&rule).unwrap();
        assert!(yaml.contains("scheme: https"), "got:\n{yaml}");
        assert!(yaml.contains("tlsTerminate: true"), "got:\n{yaml}");
        assert!(yaml.contains("path: /api/v4/**"), "got:\n{yaml}");
        let parsed: RouteRule = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, rule);
    }

    #[test]
    fn a_binary_scoped_route_rule_round_trips_with_the_core_wire_name() {
        let rule = RouteRule {
            match_pattern: "git.example.test".into(),
            verdict: Verdict::Allow,
            transport: Transport::Direct,
            scheme: None,
            description: None,
            tls_terminate: false,
            rules: Vec::new(),
            binaries: Some(vec!["/usr/bin/git".into()]),
        };
        let yaml = serde_yaml::to_string(&rule).unwrap();
        assert!(yaml.contains("binaries:"), "got:\n{yaml}");
        assert!(yaml.contains("- /usr/bin/git"), "got:\n{yaml}");
        let parsed: RouteRule = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, rule);
    }

    #[test]
    fn an_unscoped_route_rule_omits_the_binaries_key() {
        let yaml = serde_yaml::to_string(&RouteRule::allow_host("api.example.test")).unwrap();
        assert!(
            !yaml.contains("binaries"),
            "sandbox-core reads an empty binaries list as `matches no caller`, so a rule open to every caller must omit the key:\n{yaml}"
        );
    }

    #[test]
    fn load_or_default_accepts_a_rule_scoped_to_an_absolute_binary() {
        let dir = TempDir::new().unwrap();
        let path = decisions_file(
            &dir,
            "\
egress:
  http:
    - match: git.example.test
      verdict: allow
      binaries:
        - /usr/bin/git
",
        );
        let p = Policy::load_or_default(&path).unwrap();
        assert_eq!(
            p.network.egress.http[0].binaries,
            Some(vec!["/usr/bin/git".to_string()])
        );
    }

    #[test]
    fn load_or_default_rejects_an_empty_binaries_filter() {
        let dir = TempDir::new().unwrap();
        let path = decisions_file(
            &dir,
            "\
egress:
  http:
    - match: git.example.test
      verdict: allow
      binaries: []
",
        );
        let err = Policy::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("git.example.test")
                && err.to_string().contains("matches no caller"),
            "got: {err}"
        );
    }

    #[test]
    fn load_or_default_rejects_a_relative_binary_path() {
        let dir = TempDir::new().unwrap();
        let path = decisions_file(
            &dir,
            "\
egress:
  http:
    - match: git.example.test
      verdict: allow
      binaries:
        - /usr/bin/git
        - git
",
        );
        let err = Policy::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("\"git\"")
                && err.to_string().contains("is not an absolute path")
                && err.to_string().contains("/proc/<pid>/exe"),
            "got: {err}"
        );
    }

    fn scoped_to(binary: &str) -> RouteRule {
        RouteRule {
            binaries: Some(vec![binary.to_string()]),
            ..RouteRule::allow_host("git.example.test")
        }
    }

    #[test]
    fn validate_binaries_rejects_a_path_climbing_through_a_parent_segment() {
        let err = scoped_to("/usr/bin/../bin/git")
            .validate_binaries()
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("\"..\" segment"),
            "core compares path components, so `..` survives and never equals a kernel-resolved exe path: {err}"
        );
    }

    #[test]
    fn validate_binaries_rejects_a_path_naming_no_binary() {
        let err = scoped_to("/").validate_binaries().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("names no binary"), "got: {err}");
    }

    #[test]
    fn validate_binaries_accepts_the_separators_a_kernel_path_compares_equal_to() {
        for equivalent in [
            "/usr/bin/git/",
            "//usr/bin/git",
            "/usr//bin/git",
            "/usr/./bin/git",
        ] {
            assert!(
                scoped_to(equivalent).validate_binaries().is_ok(),
                "{equivalent} compares equal to /usr/bin/git as a Path, so refusing it would be a false error"
            );
        }
    }

    #[test]
    fn an_http_rule_omits_absent_method_and_path() {
        let any = HttpRule {
            method: None,
            path: None,
        };
        let yaml = serde_yaml::to_string(&any).unwrap();
        assert!(!yaml.contains("method"), "got:\n{yaml}");
        assert!(!yaml.contains("path"), "got:\n{yaml}");
        let parsed: HttpRule = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, any);
    }

    #[test]
    fn verdicts_serialize_in_lowercase() {
        assert_eq!(
            serde_yaml::to_string(&Verdict::Allow).unwrap().trim(),
            "allow"
        );
        assert_eq!(
            serde_yaml::to_string(&Verdict::Deny).unwrap().trim(),
            "deny"
        );
    }

    #[test]
    fn transports_serialize_in_lowercase() {
        assert_eq!(
            serde_yaml::to_string(&Transport::Upstream).unwrap().trim(),
            "upstream"
        );
        assert_eq!(
            serde_yaml::to_string(&Transport::Direct).unwrap().trim(),
            "direct"
        );
    }

    #[test]
    fn load_or_default_returns_default_when_file_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("no-such-file.yaml");
        let p = Policy::load_or_default(&path).unwrap();
        assert_eq!(p, Policy::default());
    }

    #[test]
    fn load_or_default_reads_existing_yaml() {
        let dir = TempDir::new().unwrap();
        let path = decisions_file(
            &dir,
            "\
egress:
  http:
    - match: api.linear.app
      verdict: allow
",
        );
        let p = Policy::load_or_default(&path).unwrap();
        assert_eq!(p.network.egress.http.len(), 1);
        assert_eq!(p.network.egress.http[0].match_pattern, "api.linear.app");
        assert_eq!(p.network.egress.http[0].verdict, Verdict::Allow);
    }

    #[test]
    fn load_or_default_rejects_a_stale_key_inside_egress_rather_than_reading_zero_rules() {
        let dir = TempDir::new().unwrap();
        let path = decisions_file(
            &dir,
            "\
egress:
  allowedRoutes:
    - match: api.linear.app
      verdict: allow
",
        );
        let err = Policy::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("allowedRoutes"),
            "a key nothing reads must name itself in the error, not load as a decision that decides nothing: {err}"
        );
    }

    #[test]
    fn an_unknown_egress_table_is_a_parse_error_rather_than_an_empty_one() {
        let err = serde_yaml::from_str::<NetworkPolicy>(
            "egress:\n  udp:\n    - match: db.example:5432\n",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("udp"),
            "an unrecognised egress table must fail the load, not publish zero rules: {err}"
        );
    }

    fn tcp_allow(pattern: &str) -> TcpEgressRule {
        TcpEgressRule::allow_destination(pattern)
    }

    #[test]
    fn a_tcp_rule_round_trips_with_sandbox_core_wire_names() {
        let rule = TcpEgressRule {
            match_pattern: "db.internal:5432".into(),
            verdict: Verdict::Allow,
            binaries: Some(vec!["/usr/bin/psql".into()]),
            description: Some("project database".into()),
        };
        let yaml = serde_yaml::to_string(&rule).unwrap();
        assert!(yaml.contains("match: db.internal:5432"), "got:\n{yaml}");
        assert!(!yaml.contains("matchPattern"), "rust ident leaked:\n{yaml}");
        assert!(
            !yaml.contains("transport"),
            "raw egress is always direct, so core's tcp rule has no transport field and ignores one we send — a key that reads as a routing choice and changes nothing:\n{yaml}"
        );
        assert_eq!(serde_yaml::from_str::<TcpEgressRule>(&yaml).unwrap(), rule);
    }

    #[test]
    fn a_transport_on_a_tcp_rule_is_a_parse_error_rather_than_a_silent_extra_key() {
        let err = serde_yaml::from_str::<TcpEgressRule>(
            "match: db.internal:5432\nverdict: allow\ntransport: upstream\n",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("transport"),
            "core ignores the key rather than refusing it, so a file routing raw egress through an upstream proxy would load and do nothing of the sort; the author has to hear that here: {err}"
        );
    }

    #[test]
    fn a_plain_tcp_rule_omits_the_optional_keys() {
        let yaml = serde_yaml::to_string(&tcp_allow("db.internal:5432")).unwrap();
        assert!(
            !yaml.contains("binaries") && !yaml.contains("description"),
            "a plain tcp allow must stay minimal:\n{yaml}"
        );
    }

    #[test]
    fn a_policy_carrying_both_egress_tables_round_trips() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("api.example.test"));
        p.network.egress.tcp.push(tcp_allow("db.internal:5432"));
        let yaml = serde_yaml::to_string(&p).unwrap();
        assert_eq!(serde_yaml::from_str::<Policy>(&yaml).unwrap(), p);
    }

    #[test]
    fn load_or_default_reads_a_tcp_rule() {
        let dir = TempDir::new().unwrap();
        let path = decisions_file(
            &dir,
            "egress:\n  tcp:\n    - match: db.internal:5432\n      verdict: allow\n",
        );
        let p = Policy::load_or_default(&path).unwrap();
        assert_eq!(p.network.egress.tcp, vec![tcp_allow("db.internal:5432")]);
    }

    #[test]
    fn a_portless_tcp_pattern_is_refused_at_load_rather_than_in_the_guest() {
        for pattern in ["db.internal", "10.0.0.0/8", "*.rds.example", "db:notaport"] {
            let err = serde_yaml::from_str::<NetworkPolicy>(&format!(
                "egress:\n  tcp:\n    - match: \"{pattern}\"\n      verdict: allow\n"
            ))
            .unwrap_err()
            .to_string();
            assert!(
                err.contains(&format!(
                    "egress.tcp rule \"{pattern}\" must specify a port, e.g. \"host:443\" or \"10.0.0.0/24:443\""
                )),
                "core force-denies the entire policy on this pattern, so the author has to hear it here: {err}"
            );
        }
    }

    #[test]
    fn a_tcp_pattern_on_port_zero_is_refused_at_load() {
        let err = serde_yaml::from_str::<NetworkPolicy>(
            "egress:\n  tcp:\n    - match: db.internal:0\n      verdict: allow\n",
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains(
                "egress.tcp rule \"db.internal:0\": port 0 is not a valid destination port"
            ),
            "got: {err}"
        );
    }

    #[test]
    fn an_unbracketed_ipv6_tcp_pattern_is_refused_at_load() {
        let err = serde_yaml::from_str::<NetworkPolicy>(
            "egress:\n  tcp:\n    - match: \"2001:db8::1:5432\"\n      verdict: allow\n",
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("ambiguous IPv6 address without brackets"),
            "got: {err}"
        );
    }

    #[test]
    fn bracketed_ipv6_and_cidr_tcp_patterns_load() {
        for pattern in [
            "[2001:db8::1]:5432",
            "[2001:db8::/32]:5432",
            "10.20.0.0/24:5432",
            "10.20.5.10:6379",
            // ipnet accepts leading-zero octets, so refusing this locally would be a false refusal.
            "[010.0.0.0/8]:443",
        ] {
            let net: NetworkPolicy = serde_yaml::from_str(&format!(
                "egress:\n  tcp:\n    - match: \"{pattern}\"\n      verdict: allow\n"
            ))
            .unwrap_or_else(|e| panic!("core accepts {pattern}, so we must too: {e}"));
            assert_eq!(net.egress.tcp[0].match_pattern, pattern);
        }
    }

    #[test]
    fn a_malformed_bracketed_tcp_pattern_is_refused_at_load() {
        for (pattern, needle) in [
            ("[2001:db8::1]", "invalid bracketed address"),
            ("[2001:db8::1]:notaport", "invalid port in"),
            ("[nonsense/24]:5432", "invalid CIDR in"),
            // ipnet rejects these prefix spellings, and a rule we accept that core cannot parse force-denies the whole policy in the guest.
            ("[10.0.0.0/032]:5432", "invalid CIDR in"),
            ("[2001:db8::/+32]:5432", "invalid CIDR in"),
            ("[2001:db8::/0128]:443", "invalid CIDR in"),
        ] {
            let err = serde_yaml::from_str::<NetworkPolicy>(&format!(
                "egress:\n  tcp:\n    - match: \"{pattern}\"\n      verdict: allow\n"
            ))
            .unwrap_err()
            .to_string();
            assert!(err.contains(needle), "for {pattern}, got: {err}");
        }
    }

    #[test]
    fn an_unbracketed_tcp_pattern_with_a_broken_prefix_is_refused_at_load() {
        for pattern in ["10.0.0.0/032:5432", "10.0.0.0/+8:443", "10.0.0.0/0128:443"] {
            let err = serde_yaml::from_str::<NetworkPolicy>(&format!(
                "egress:\n  tcp:\n    - match: \"{pattern}\"\n      verdict: allow\n"
            ))
            .unwrap_err()
            .to_string();
            assert!(
                err.contains(&format!("invalid CIDR in {pattern}")),
                "core reads a prefix ipnet rejects as a hostname, so the rule loads and then matches nothing: {err}"
            );
        }
    }

    #[test]
    fn an_empty_binaries_filter_is_refused_at_load() {
        let err = serde_yaml::from_str::<NetworkPolicy>(
            "egress:\n  tcp:\n    - match: db.internal:5432\n      verdict: allow\n      binaries: []\n",
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("binaries filter is empty"),
            "an empty filter matches no caller and silently turns the allow into a deny: {err}"
        );
    }

    #[test]
    fn a_relative_binaries_entry_is_refused_at_load() {
        let err = serde_yaml::from_str::<NetworkPolicy>(
            "egress:\n  tcp:\n    - match: db.internal:5432\n      verdict: allow\n      binaries: [psql]\n",
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("is not an absolute path"),
            "entries are matched against /proc/<pid>/exe, so a relative path can never match: {err}"
        );
    }

    #[test]
    fn a_raw_binaries_entry_that_can_never_match_a_caller_is_refused_at_load() {
        for (entry, why) in [
            ("psql", "is not an absolute path"),
            ("/usr/bin/../bin/psql", "\"..\" segment"),
            ("/", "names no binary"),
        ] {
            let err = serde_yaml::from_str::<NetworkPolicy>(&format!(
                "egress:\n  tcp:\n    - match: db.internal:5432\n      verdict: allow\n      binaries: [\"{entry}\"]\n"
            ))
            .unwrap_err()
            .to_string();
            assert!(
                err.contains(why),
                "a scope the kernel-resolved exe path can never equal silently denies every caller: {err}"
            );
        }
    }

    #[test]
    fn an_approved_route_appends_when_no_rule_asks() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("first.example"));
        p.add_approved_rule(RouteRule::allow_host("second.example"));
        assert_eq!(
            p.network.egress.http.last().unwrap().match_pattern,
            "second.example"
        );
    }

    #[test]
    fn approving_the_same_route_twice_does_not_duplicate_it() {
        let mut p = Policy::default();
        p.add_approved_rule(RouteRule::allow_host("api.example.test"));
        p.add_approved_rule(RouteRule::allow_host("api.example.test"));
        assert_eq!(p.network.egress.http.len(), 1);
    }

    #[test]
    fn an_approved_tcp_rule_appends_when_no_rule_asks() {
        let mut p = Policy::default();
        p.network
            .egress
            .tcp
            .push(TcpEgressRule::deny_destination("db.internal:5432"));
        p.add_approved_tcp_rule(TcpEgressRule::allow_destination("cache.internal:6379"));
        assert_eq!(
            p.network.egress.tcp.last().unwrap().match_pattern,
            "cache.internal:6379"
        );
    }

    #[test]
    fn approving_the_same_tcp_rule_twice_does_not_duplicate_it() {
        let mut p = Policy::default();
        p.add_approved_tcp_rule(TcpEgressRule::allow_destination("db.internal:5432"));
        p.add_approved_tcp_rule(TcpEgressRule::allow_destination("db.internal:5432"));
        assert_eq!(p.network.egress.tcp.len(), 1);
    }

    // Repositioning a rule the author placed themselves is more surprising than one more card, so the dedup wins, the prompt comes back — and the developer is told, because a silent "always" reads as remembered.
    #[test]
    fn an_approval_matching_a_rule_stranded_behind_a_deny_is_reported_as_unreachable() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::deny_host("*.example.test"));
        p.add_rule(RouteRule::allow_host("api.example.test"));
        assert_eq!(
            p.add_approved_rule(RouteRule::allow_host("api.example.test")),
            Approval::Unreachable("*.example.test".into()),
            "the gate stops at the deny, so the allow behind it never fires and the card comes back on the next request"
        );
        assert_eq!(
            p.network
                .egress
                .http
                .iter()
                .map(|r| r.match_pattern.as_str())
                .collect::<Vec<_>>(),
            vec!["*.example.test", "api.example.test"],
            "a rule the author placed is not moved on their behalf"
        );
    }

    #[test]
    fn an_approval_matching_a_raw_rule_stranded_behind_a_deny_is_reported_as_unreachable() {
        let mut p = Policy::default();
        p.network
            .egress
            .tcp
            .push(TcpEgressRule::deny_destination("db.internal:5432"));
        p.network
            .egress
            .tcp
            .push(TcpEgressRule::allow_destination("db.internal:5432"));
        assert_eq!(
            p.add_approved_tcp_rule(TcpEgressRule::allow_destination("db.internal:5432")),
            Approval::Unreachable("db.internal:5432".into())
        );
        assert_eq!(p.network.egress.tcp.len(), 2);
    }

    #[test]
    fn an_approved_route_a_standing_rule_already_decides_is_not_written_at_all() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("*.example.test"));
        assert_eq!(
            p.add_approved_rule(RouteRule::deny_host("api.example.test")),
            Approval::Shadowed("*.example.test".into()),
            "nothing asked, so nothing is being answered — moving ahead of a standing allow would decide more than was approved"
        );
        assert_eq!(
            p.network
                .egress
                .http
                .iter()
                .map(|r| r.match_pattern.as_str())
                .collect::<Vec<_>>(),
            vec!["*.example.test"],
            "the gate stops at the standing allow, so a deny written behind it is a line that never fires and a developer who thinks it does"
        );
    }

    #[test]
    fn a_second_approval_contradicting_the_rule_the_first_one_wrote_is_reported_not_written() {
        let mut p = Policy::default();
        assert_eq!(
            p.add_approved_rule(RouteRule::allow_host("api.example.test")),
            Approval::Stands
        );
        assert_eq!(
            p.add_approved_rule(RouteRule::deny_host("api.example.test")),
            Approval::Shadowed("api.example.test".into()),
            "one card is raised per request, so a second answer can arrive after the first has already written the rule that now decides"
        );
        assert_eq!(
            p.network
                .egress
                .http
                .iter()
                .map(|r| r.verdict)
                .collect::<Vec<_>>(),
            vec![Verdict::Allow],
            "the contradicting deny is reported, not written behind the allow where it would never fire"
        );
    }

    #[test]
    fn an_approval_a_standing_rule_already_grants_stands_without_a_word() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("*.example.test"));
        assert_eq!(
            p.add_approved_rule(RouteRule::allow_host("api.example.test")),
            Approval::Stands,
            "the file already does what the developer just asked for; there is nothing to report and nothing to add"
        );
        assert_eq!(p.network.egress.http.len(), 1);
    }

    #[test]
    fn an_approved_tcp_rule_a_standing_raw_rule_already_decides_is_not_written_at_all() {
        let mut p = Policy::default();
        p.network
            .egress
            .tcp
            .push(TcpEgressRule::allow_destination("10.0.0.0/24:5432"));
        assert_eq!(
            p.add_approved_tcp_rule(TcpEgressRule::deny_destination("10.0.0.5:5432")),
            Approval::Shadowed("10.0.0.0/24:5432".into()),
            "a raw deny behind a raw allow the gate reaches first never blocks anything"
        );
        assert_eq!(p.network.egress.tcp.len(), 1);
    }

    #[test]
    fn add_rule_still_appends_so_a_hand_written_order_is_preserved() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::deny_host("api.example.test"));
        p.add_rule(RouteRule::allow_host("api.example.test"));
        assert_eq!(
            p.network
                .egress
                .http
                .iter()
                .map(|r| r.verdict)
                .collect::<Vec<_>>(),
            vec![Verdict::Deny, Verdict::Allow],
            "only an approval-derived rule jumps the queue; an author's order is theirs"
        );
    }

    #[test]
    fn a_tcp_pattern_naming_no_host_is_refused_at_load() {
        let err = serde_yaml::from_str::<NetworkPolicy>(
            "egress:\n  tcp:\n    - match: \":5432\"\n      verdict: allow\n",
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("egress.tcp rule \":5432\" names no host before its port"),
            "a portless host is a destination core cannot match, and one it cannot parse force-denies the whole policy: {err}"
        );
    }

    #[test]
    fn a_bracketed_tcp_pattern_naming_no_host_is_refused_at_load() {
        let err = serde_yaml::from_str::<NetworkPolicy>(
            "egress:\n  tcp:\n    - match: \"[]:5432\"\n      verdict: allow\n",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("names no host before its port"), "got: {err}");
    }

    #[test]
    fn a_tcp_port_outside_the_range_says_so_rather_than_asking_for_a_port() {
        for pattern in ["db.internal:65536", "[2001:db8::1]:99999"] {
            let err = serde_yaml::from_str::<NetworkPolicy>(&format!(
                "egress:\n  tcp:\n    - match: \"{pattern}\"\n      verdict: allow\n"
            ))
            .unwrap_err()
            .to_string();
            assert!(
                err.contains("is not a valid port number (1-65535)"),
                "the author named a port, so sending them to look for a missing one wastes the error: {err}"
            );
        }
    }

    #[test]
    fn an_egress_table_deserialized_on_its_own_still_refuses_an_unenforceable_tcp_rule() {
        let err =
            serde_yaml::from_str::<Egress>("tcp:\n  - match: db.internal\n    verdict: allow\n")
                .unwrap_err()
                .to_string();
        assert!(
            err.contains("must specify a port"),
            "the table owning the rules is where they are checked, so no caller can reach around the check: {err}"
        );
    }

    #[test]
    fn a_valid_tcp_rule_validates_clean() {
        assert_eq!(tcp_allow("db.internal:5432").validate(), Ok(()));
    }

    #[test]
    fn an_absolute_binaries_entry_validates_clean() {
        let rule = TcpEgressRule {
            binaries: Some(vec!["/usr/bin/psql".into()]),
            ..tcp_allow("db.internal:5432")
        };
        assert_eq!(
            rule.validate(),
            Ok(()),
            "scoping a raw allow to one caller is the least-privilege shape; it must not be refused"
        );
    }

    #[test]
    fn tcp_rule_constructors_carry_the_asked_for_verdict() {
        assert_eq!(
            TcpEgressRule::allow_destination("db.internal:5432").verdict,
            Verdict::Allow
        );
        assert_eq!(
            TcpEgressRule::deny_destination("db.internal:5432").verdict,
            Verdict::Deny
        );
    }

    #[test]
    fn saving_emits_the_egress_table_the_guest_routes_on() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("api.linear.app"));
        let yaml = serde_yaml::to_string(&p).unwrap();
        assert!(yaml.contains("egress:"), "got:\n{yaml}");
        assert!(yaml.contains("http:"), "got:\n{yaml}");
    }

    #[test]
    fn an_empty_policy_still_writes_the_egress_tables_the_guest_reads() {
        let yaml = serde_yaml::to_string(&Policy::default()).unwrap();
        assert!(
            yaml.contains("http: []") && yaml.contains("tcp: []"),
            "the guest reads egress.http and egress.tcp; omitting either hands its destinations to the wrong table:\n{yaml}"
        );
    }

    #[test]
    fn load_or_default_rejects_an_upstream_route_transport() {
        let dir = TempDir::new().unwrap();
        let path = decisions_file(
            &dir,
            "\
egress:
  http:
    - match: api.example.test
      verdict: allow
      transport: upstream
",
        );
        let err = Policy::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string()
                .contains("upstream transport isn't supported in the local sandbox"),
            "got: {err}"
        );
    }

    #[test]
    fn load_or_default_surfaces_non_not_found_io_errors() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("is-actually-a-dir");
        fs::create_dir(&path).unwrap();
        let err = Policy::load_or_default(&path).unwrap_err();
        assert_ne!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn load_or_default_surfaces_invalid_yaml_as_io_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("broken.yaml");
        fs::write(&path, "apiVersion: [\n").unwrap();
        let err = Policy::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn save_atomic_writes_file_readable_by_load_or_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("decisions.yaml");
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("api.linear.app"));

        p.save_atomic(&path).unwrap();
        let reloaded = Policy::load_or_default(&path).unwrap();
        assert_eq!(p.network, reloaded.network);
    }

    #[test]
    fn save_atomic_creates_parent_directory() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/dir/decisions.yaml");
        Policy::default().save_atomic(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_atomic_leaves_no_tmp_file_on_success() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("decisions.yaml");
        Policy::default().save_atomic(&path).unwrap();
        let tmp = path.with_extension("yaml.tmp");
        assert!(!tmp.exists(), "tmp file should be renamed away");
    }

    #[test]
    fn an_approval_for_a_host_the_developer_already_allowed_keeps_their_own_entry() {
        // The note belongs to the run, so an entry somebody typed without one has to still read as the answer they already gave rather than as a near-duplicate to append.
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("docs.some-vendor.example"));

        let approval =
            p.add_approved_rule(RouteRule::allow_host("docs.some-vendor.example").approved());

        assert_eq!(approval, Approval::Stands);
        assert_eq!(
            p.network.egress.http,
            [RouteRule::allow_host("docs.some-vendor.example")],
            "the entry they wrote keeps its own words, and nothing is written beside it"
        );
    }

    #[test]
    fn an_approval_for_a_raw_destination_the_developer_already_allowed_keeps_their_own_entry() {
        let mut p = Policy::default();
        p.network.egress.tcp.push(TcpEgressRule::allow_destination(
            "db.some-vendor.example:5432",
        ));

        let approval = p.add_approved_tcp_rule(
            TcpEgressRule::allow_destination("db.some-vendor.example:5432").approved(),
        );

        assert_eq!(approval, Approval::Stands);
        assert_eq!(
            p.network.egress.tcp,
            [TcpEgressRule::allow_destination(
                "db.some-vendor.example:5432"
            )],
            "the entry they wrote keeps its own words, and nothing is written beside it"
        );
    }

    #[test]
    fn asking_again_takes_back_the_entry_the_approval_wrote() {
        let mut p = Policy::default();
        p.add_approved_rule(RouteRule::allow_host("api.linear.app").approved());

        assert!(p.remove_approved_rule("api.linear.app"));
        assert!(
            p.network.egress.http.is_empty(),
            "the destination is undecided again, so the gate has to ask"
        );
    }

    #[test]
    fn asking_again_leaves_the_entry_the_author_typed() {
        // A rule somebody wrote by hand says what the project decided; an answer given in a window is not a licence to delete it.
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("api.linear.app"));

        assert!(!p.remove_approved_rule("api.linear.app"));
        assert_eq!(
            p.network.egress.http,
            [RouteRule::allow_host("api.linear.app")]
        );
    }

    #[test]
    fn asking_again_leaves_every_other_destination_where_it_is() {
        let mut p = Policy::default();
        p.add_approved_rule(RouteRule::allow_host("api.linear.app").approved());
        p.add_approved_rule(RouteRule::allow_host("api.github.com").approved());

        assert!(p.remove_approved_rule("api.linear.app"));
        assert_eq!(
            p.network.egress.http,
            [RouteRule::allow_host("api.github.com").approved()]
        );
    }

    #[test]
    fn asking_again_about_a_destination_nothing_decided_changes_nothing() {
        let mut p = Policy::default();

        assert!(!p.remove_approved_rule("api.linear.app"));
        assert!(p.network.egress.http.is_empty());
    }

    #[test]
    fn asking_again_takes_back_the_raw_entry_the_approval_wrote() {
        let mut p = Policy::default();
        p.add_approved_tcp_rule(TcpEgressRule::allow_destination("db.internal:5432").approved());

        assert!(p.remove_approved_tcp_rule("db.internal:5432"));
        assert!(p.network.egress.tcp.is_empty());
    }

    #[test]
    fn asking_again_leaves_the_raw_entry_the_author_typed() {
        let mut p = Policy::default();
        p.network
            .egress
            .tcp
            .push(TcpEgressRule::allow_destination("db.internal:5432"));

        assert!(!p.remove_approved_tcp_rule("db.internal:5432"));
        assert_eq!(
            p.network.egress.tcp,
            [TcpEgressRule::allow_destination("db.internal:5432")]
        );
    }

    #[test]
    fn save_atomic_does_not_follow_a_symlink_planted_at_the_tmp_path() {
        // The service writes this file unattended on every approval, so a symlink planted at the tmp path would turn a click into a write of whatever it points at.
        let dir = TempDir::new().unwrap();
        let victim = dir.path().join("victim");
        let victim_contents = b"victim-data-must-survive";
        fs::write(&victim, victim_contents).unwrap();
        let path = dir.path().join("decisions.yaml");
        std::os::unix::fs::symlink(&victim, path.with_extension("yaml.tmp")).unwrap();

        let _ = Policy::default().save_atomic(&path);

        assert_eq!(
            fs::read(&victim).unwrap(),
            victim_contents,
            "a symlink at the tmp path must not redirect the decisions write"
        );
    }

    #[test]
    fn save_atomic_leaves_the_mode_umask_gives_a_file_the_developer_commits() {
        // §8.2 makes this a file a project commits and shares, so the 0600 a secret sidecar takes would lock out a second account for no gain — compared against a plain write in the same directory, so the assertion holds under any umask.
        let dir = TempDir::new().unwrap();
        let reference = dir.path().join("reference");
        fs::write(&reference, b"").unwrap();
        let path = dir.path().join("decisions.yaml");

        Policy::default().save_atomic(&path).unwrap();

        let mode = |p: &Path| {
            std::os::unix::fs::PermissionsExt::mode(&fs::metadata(p).unwrap().permissions()) & 0o777
        };
        assert_eq!(
            mode(&path),
            mode(&reference),
            "the decisions file must be as readable as any file the developer writes here"
        );
    }

    #[test]
    fn file_policy_store_save_writes_yaml_readable_by_load_or_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("decisions.yaml");
        let store = FilePolicyStore::new(path.clone());

        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("api.linear.app"));
        store.save(&p).unwrap();

        let reloaded = Policy::load_or_default(&path).unwrap();
        assert_eq!(reloaded.network, p.network);
    }

    #[test]
    fn file_policy_store_save_to_unwritable_parent_surfaces_error() {
        let dir = TempDir::new().unwrap();
        let unwritable = dir.path().join("file-not-a-dir");
        fs::write(&unwritable, b"").unwrap();
        let path = unwritable.join("nested/decisions.yaml");
        let store = FilePolicyStore::new(path);

        let err = store.save(&Policy::default()).unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn add_rule_appends_to_the_http_egress_table_in_order() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("a"));
        p.add_rule(RouteRule::deny_host("b"));
        assert_eq!(p.network.egress.http.len(), 2);
        assert_eq!(p.network.egress.http[0].match_pattern, "a");
        assert_eq!(p.network.egress.http[0].verdict, Verdict::Allow);
        assert_eq!(p.network.egress.http[1].match_pattern, "b");
        assert_eq!(p.network.egress.http[1].verdict, Verdict::Deny);
    }

    #[test]
    fn add_rule_skips_an_exact_duplicate() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("huggingface.co"));
        p.add_rule(RouteRule::allow_host("huggingface.co"));
        assert_eq!(p.network.egress.http.len(), 1);
    }

    #[test]
    fn add_rule_keeps_a_same_host_rule_that_differs() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("example.com"));
        p.add_rule(RouteRule::deny_host("example.com"));
        assert_eq!(p.network.egress.http.len(), 2);
    }

    #[test]
    fn add_rule_keeps_two_rules_that_differ_only_in_their_binaries() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("git.example.test"));
        p.add_rule(RouteRule {
            binaries: Some(vec!["/usr/bin/git".into()]),
            ..RouteRule::allow_host("git.example.test")
        });
        assert_eq!(
            p.network.egress.http.len(),
            2,
            "a binary-scoped grant is a different grant, not a duplicate of the open one"
        );
    }

    #[test]
    fn the_decisions_file_is_the_mixin_the_specification_says_it_is() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("decisions.yaml");
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("docs.some-vendor.example"));
        p.save_atomic(&path).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("apiVersion: lns.run/v1") && text.contains("kind: mixin"),
            "§8.1 records a decision in the grammar the rest of the document already defines; got:\n{text}"
        );
        assert!(
            text.contains("egress:") && !text.contains("network:"),
            "§3.1.6 names the block `egress`, directly under `spec`; got:\n{text}"
        );
        assert_eq!(
            Policy::load_or_default(&path).unwrap().network.egress.http,
            p.network.egress.http,
            "what the run wrote back is what the next run reads"
        );
    }

    #[test]
    fn the_document_is_named_for_the_file_it_is_written_to() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("decisions.yaml");
        Policy::default().save_atomic(&path).unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("name: decisions"),
            "§2 requires a name on every document, and nobody is here to choose one"
        );
    }

    #[test]
    fn a_name_no_dns_label_could_be_is_written_as_one() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("My_Decisions.yaml");
        Policy::default().save_atomic(&path).unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("name: my-decisions"),
            "a `--policy` path the developer chose still has to produce a document §2 accepts"
        );
    }

    #[test]
    fn a_file_stem_holding_no_label_character_still_produces_a_name() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("__.yaml");
        Policy::default().save_atomic(&path).unwrap();
        assert!(
            fs::read_to_string(&path).unwrap().contains("name: local"),
            "§2 requires a name, so a file whose stem spells none has to fall back to one"
        );
    }

    #[test]
    fn a_file_stem_longer_than_a_label_is_cut_to_one() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(format!("{}-tail.yaml", "a".repeat(70)));
        Policy::default().save_atomic(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let name = text
            .lines()
            .find_map(|line| line.strip_prefix("name: "))
            .expect("the document names itself");
        assert_eq!(
            name,
            "a".repeat(63),
            "§2 caps a label at 63 characters, and a path the developer chose can be longer"
        );
    }

    #[test]
    fn a_block_the_run_never_writes_survives_the_run_writing_one_it_does() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("decisions.yaml");
        fs::write(
            &path,
            "apiVersion: lns.run/v1\nkind: mixin\nname: decisions\nspec:\n  mixins:\n    - ghcr.io/team/base@sha256:abc\n  egress:\n    http: []\n",
        )
        .unwrap();

        let mut p = Policy::load_or_default(&path).unwrap();
        p.add_rule(RouteRule::allow_host("api.example.test"));
        p.save_atomic(&path).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("ghcr.io/team/base@sha256:abc"),
            "an approval appends an egress entry; it must not delete what the developer wrote by hand:\n{text}"
        );
        assert!(text.contains("api.example.test"), "got:\n{text}");
    }

    #[test]
    fn a_document_of_another_kind_is_refused_rather_than_read_as_decisions() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("decisions.yaml");
        fs::write(
            &path,
            "apiVersion: lns.run/v1\nkind: sandbox\nname: something-else\nspec:\n  egress:\n    http: []\n",
        )
        .unwrap();
        let err = Policy::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("mixin"),
            "the file is the developer's own, so a wrong kind is loud rather than silently empty: {err}"
        );
    }

    #[test]
    fn a_document_named_what_no_document_may_be_named_is_refused() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("decisions.yaml");
        fs::write(
            &path,
            "apiVersion: lns.run/v1\nkind: mixin\nname: Not_A_Label!\nspec:\n  egress:\n    http: []\n",
        )
        .unwrap();
        let err = Policy::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("Not_A_Label!"),
            "the resolver refuses this document too, so the two readers of one grammar have to agree: {err}"
        );
    }

    #[test]
    fn a_name_is_a_label_or_it_is_not_a_name() {
        for valid in ["a", "9", "team-egress", &"a".repeat(63)] {
            assert!(is_dns_label(valid), "{valid} is a label");
        }
        for invalid in ["", "-a", "a-", "aBc", "a_b", "a.b", &"a".repeat(64)] {
            assert!(!is_dns_label(invalid), "{invalid} is not a label");
        }
    }

    #[test]
    fn a_document_of_another_api_version_is_refused() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("decisions.yaml");
        fs::write(
            &path,
            "apiVersion: lns.run/v2\nkind: mixin\nname: local\nspec:\n  egress:\n    http: []\n",
        )
        .unwrap();
        let err = Policy::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("lns.run/v1"), "got: {err}");
    }

    #[test]
    fn an_empty_file_is_the_mixin_nobody_wrote() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("decisions.yaml");
        fs::write(&path, "  \n").unwrap();
        assert_eq!(
            Policy::load_or_default(&path).unwrap().network,
            NetworkPolicy::default(),
            "§8.1 has the mixin exist whether or not anyone wrote it, and an editor truncates before it writes"
        );
    }
}
