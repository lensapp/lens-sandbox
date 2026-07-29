use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::matching::{port_shaped, split_destination};

pub mod connectors;
pub mod credentials;
mod env_subst;
pub mod grants;
pub mod host_bind_decisions;
pub mod matching;
pub mod providers;
pub mod registry_auth;
mod secure_file;
#[cfg(test)]
mod test_env;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    #[serde(default)]
    pub network: NetworkPolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connectors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkPolicy {
    #[serde(default)]
    pub egress: Egress,
    #[serde(default = "default_ask")]
    pub default_verdict: Verdict,
    // Always serialized: sandbox-core's schema requires transport, and a missing one fail-closes a non-deny verdict to deny in the guest.
    #[serde(default)]
    pub default_transport: Transport,
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

fn default_ask() -> Verdict {
    Verdict::Ask
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            egress: Egress::default(),
            default_verdict: Verdict::Ask,
            default_transport: Transport::Direct,
        }
    }
}

impl NetworkPolicy {
    pub fn validate_local_transport(&self) -> io::Result<()> {
        let uses_upstream = self.default_transport == Transport::Upstream
            || self
                .egress
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
        self.egress
            .http
            .iter()
            .try_for_each(RouteRule::validate_binaries)
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

/// One raw-TCP egress rule: a port-scoped destination the guest splices through untouched — no TLS interception, no HTTP rules, no credential injection.
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

/// Why a binaries entry can never name a caller, or `None` when it can. Absoluteness is judged the way the guest kernel will judge it, not the way this host would.
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

/// The destination port an `egress.tcp` `match` names, mirroring lens-sandbox-core's `parse_matcher`; a pattern that resolves to a portless matcher is an error because a raw splice is granted with no inspection at all.
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
    // An unbracketed tail that is no kind of number may be a hostname the author never meant to port-scope, so it reads as the port they left off.
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
    // A `]:` puts the tail in a position only a port can occupy, so a tail that isn't one is a broken port rather than a missing one.
    if !port_shaped(port) {
        return Err(format!("invalid port in {pattern}"));
    }
    parse_port(pattern, port)
}

/// A prefix core's CIDR parser rejects leaves the host reading as a hostname there — one that resolves to nothing and matches no connection — so a `/` that is not a range is refused rather than written as a rule that silently never fires.
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

/// The port a pattern's numeric tail names; one too large to be a port is its own error, because the author did name one and should not be sent looking for a missing colon.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Allow,
    Deny,
    Ask,
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
            Ok(text) => {
                let policy: Self = serde_yaml::from_str(&text)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                policy.network.validate_local_transport()?;
                policy.network.validate_binary_scopes()?;
                Ok(policy)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    pub fn save_atomic(&self, path: &Path) -> io::Result<()> {
        let yaml = serde_yaml::to_string(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("yaml.tmp");
        fs::write(&tmp, yaml)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn add_rule(&mut self, rule: RouteRule) {
        if !self.network.egress.http.contains(&rule) {
            self.network.egress.http.push(rule);
        }
    }

    /// Records a decision the developer just made on a held request. See [`place_approved`] for the placement and [`answering_an_ask`] for why the rule isn't built from the host alone.
    pub fn add_approved_rule(&mut self, rule: RouteRule) -> Approval {
        let shadowing = self
            .network
            .first_shadowing_rule(&rule)
            .map(|(index, shadowing)| (index, shadowing.clone()));
        let rule = match &shadowing {
            Some((_, asked)) if answering_an_ask(asked.verdict, rule.verdict) => RouteRule {
                match_pattern: rule.match_pattern.clone(),
                verdict: rule.verdict,
                description: rule.description.clone(),
                ..asked.clone()
            },
            _ => rule,
        };
        place_approved(&mut self.network.egress.http, rule, shadowing)
    }

    /// Records a decision the developer just made on a held raw-TCP request. See [`place_approved`] for the placement and [`answering_an_ask`] for why the rule isn't built from the destination alone.
    pub fn add_approved_tcp_rule(&mut self, rule: TcpEgressRule) -> Approval {
        let shadowing = self
            .network
            .first_shadowing_tcp_rule(&rule)
            .map(|(index, shadowing)| (index, shadowing.clone()));
        let rule = match &shadowing {
            Some((_, asked)) if answering_an_ask(asked.verdict, rule.verdict) => TcpEgressRule {
                match_pattern: rule.match_pattern.clone(),
                verdict: rule.verdict,
                description: rule.description.clone(),
                ..asked.clone()
            },
            _ => rule,
        };
        place_approved(&mut self.network.egress.tcp, rule, shadowing)
    }

    pub fn connect(&mut self, id: impl Into<String>) {
        let id = id.into();
        if !self.connectors.contains(&id) {
            self.connectors.push(id);
        }
    }

    pub fn disconnect(&mut self, id: &str) -> bool {
        let before = self.connectors.len();
        self.connectors.retain(|i| i != id);
        self.connectors.len() != before
    }
}

/// What became of a decision meant to outlive the request it was made on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Approval {
    /// The decision is in force from here on, either as a rule just written or as one the file already held.
    Stands,
    /// Nothing was written: the named rule already decides every request this one could match, and the gate stops at the first match — so the decision applied to its own request and no further.
    Shadowed(String),
    /// Nothing was written: the table already holds this exact rule, but behind the named rule the gate reaches first, so the copy never fires. Repositioning a rule the author placed themselves is a bigger surprise than the prompt coming back, so the reordering is theirs to make.
    Unreachable(String),
}

/// Whether the rule being written is the answer to a standing `ask`. An approval says "stop asking", not "treat this destination differently", so the rule it writes carries the asking rule's TLS termination, request filter and binary scope rather than silently lifting them — but never its `match`, which the card never showed and which would grant every other destination the ask covered. A deny narrows whatever the ask covered and needs nothing carried.
fn answering_an_ask(standing: Verdict, decided: Verdict) -> bool {
    standing == Verdict::Ask && decided != Verdict::Deny
}

/// A rule table entry the gate scans first-match-wins.
trait Placed: PartialEq {
    fn verdict(&self) -> Verdict;
    fn match_pattern(&self) -> &str;
}

impl Placed for RouteRule {
    fn verdict(&self) -> Verdict {
        self.verdict
    }

    fn match_pattern(&self) -> &str {
        &self.match_pattern
    }
}

impl Placed for TcpEgressRule {
    fn verdict(&self) -> Verdict {
        self.verdict
    }

    fn match_pattern(&self) -> &str {
        &self.match_pattern
    }
}

/// Places an approval-derived rule where the guest gate will actually reach it: ahead of the `ask` rule that raised the prompt, since lens-sandbox-core stops at the first match and appending behind that rule would be dead. With nothing covering it the rule appends. A rule that already *decides* the destination is neither jumped — that would grant more than the card showed — nor written behind, which would be a rule that never fires; the decision stands for its own request and the caller is told it outlived nothing.
fn place_approved<R: Placed>(
    table: &mut Vec<R>,
    rule: R,
    shadowing: Option<(usize, R)>,
) -> Approval {
    let held = table.iter().position(|existing| *existing == rule);
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
    if shadowing.verdict() == Verdict::Ask {
        table.insert(index, rule);
        return Approval::Stands;
    }
    if shadowing.verdict() == rule.verdict() {
        return Approval::Stands;
    }
    Approval::Shadowed(shadowing.match_pattern().to_string())
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

    #[test]
    fn default_policy_uses_verdict_ask_with_direct_transport() {
        let p = Policy::default();
        assert_eq!(p.network.default_verdict, Verdict::Ask);
        assert_eq!(p.network.default_transport, Transport::Direct);
        assert!(p.network.egress.http.is_empty());
    }

    #[test]
    fn a_network_section_omitting_the_defaults_parses_to_ask_and_direct() {
        let net: NetworkPolicy = serde_yaml::from_str("egress:\n  http: []\n").unwrap();
        assert_eq!(net.default_verdict, Verdict::Ask);
        assert_eq!(net.default_transport, Transport::Direct);
    }

    #[test]
    fn default_policy_yaml_roundtrip_is_lossless() {
        let p = Policy::default();
        let yaml = serde_yaml::to_string(&p).unwrap();
        let parsed: Policy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(p, parsed);
    }

    #[test]
    fn default_policy_yaml_emits_the_transport_the_guest_gate_requires() {
        let yaml = serde_yaml::to_string(&Policy::default()).unwrap();
        assert!(
            yaml.contains("defaultTransport: direct"),
            "the sandbox-core schema requires defaultTransport; omitting it fail-closes ask to deny in the guest:\n{yaml}"
        );
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
        let path = dir.path().join("lns-policy.yaml");
        let yaml = "\
network:
  egress:
    http:
      - match: git.example.test
        verdict: allow
        binaries:
          - /usr/bin/git
  defaultVerdict: ask
";
        fs::write(&path, yaml).unwrap();
        let p = Policy::load_or_default(&path).unwrap();
        assert_eq!(
            p.network.egress.http[0].binaries,
            Some(vec!["/usr/bin/git".to_string()])
        );
    }

    #[test]
    fn load_or_default_rejects_an_empty_binaries_filter() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let yaml = "\
network:
  egress:
    http:
      - match: git.example.test
        verdict: allow
        binaries: []
  defaultVerdict: ask
";
        fs::write(&path, yaml).unwrap();
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
        let path = dir.path().join("lns-policy.yaml");
        let yaml = "\
network:
  egress:
    http:
      - match: git.example.test
        verdict: allow
        binaries:
          - /usr/bin/git
          - git
  defaultVerdict: ask
";
        fs::write(&path, yaml).unwrap();
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
        assert_eq!(serde_yaml::to_string(&Verdict::Ask).unwrap().trim(), "ask");
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
        let path = dir.path().join("lns-policy.yaml");
        let yaml = "\
network:
  egress:
    http:
      - match: api.linear.app
        verdict: allow
  defaultVerdict: ask
";
        fs::write(&path, yaml).unwrap();
        let p = Policy::load_or_default(&path).unwrap();
        assert_eq!(p.network.egress.http.len(), 1);
        assert_eq!(p.network.egress.http[0].match_pattern, "api.linear.app");
        assert_eq!(p.network.egress.http[0].verdict, Verdict::Allow);
    }

    #[test]
    fn load_or_default_rejects_a_file_still_naming_the_removed_allowed_routes_key() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let yaml = "\
network:
  allowedRoutes:
    - match: api.linear.app
      verdict: allow
  defaultVerdict: ask
";
        fs::write(&path, yaml).unwrap();
        let err = Policy::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("allowedRoutes"),
            "a stale key must name itself in the error, not load as a policy with zero rules: {err}"
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
        let path = dir.path().join("lns-policy.yaml");
        fs::write(
            &path,
            "network:\n  egress:\n    tcp:\n      - match: db.internal:5432\n        verdict: allow\n  defaultVerdict: ask\n",
        )
        .unwrap();
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

    fn ask_route(host: &str) -> RouteRule {
        RouteRule {
            match_pattern: host.into(),
            verdict: Verdict::Ask,
            transport: Transport::Direct,
            scheme: None,
            description: None,
            tls_terminate: false,
            rules: Vec::new(),
            binaries: None,
        }
    }

    #[test]
    fn an_approved_route_lands_before_the_ask_rule_that_raised_the_prompt() {
        let mut p = Policy::default();
        p.add_rule(ask_route("api.example.test"));
        p.add_approved_rule(RouteRule::allow_host("api.example.test"));
        assert_eq!(
            p.network
                .egress
                .http
                .iter()
                .map(|r| r.verdict)
                .collect::<Vec<_>>(),
            vec![Verdict::Allow, Verdict::Ask],
            "the gate is first-match-wins, so an approval appended after the ask rule would never be reached"
        );
    }

    #[test]
    fn an_approved_route_keeps_the_rules_that_already_decide_ahead_of_it() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::deny_host("evil.example"));
        p.add_rule(ask_route("api.example.test"));
        p.add_approved_rule(RouteRule::allow_host("api.example.test"));
        assert_eq!(
            p.network
                .egress
                .http
                .iter()
                .map(|r| r.match_pattern.as_str())
                .collect::<Vec<_>>(),
            vec!["evil.example", "api.example.test", "api.example.test"],
            "only the undecided tail moves; a standing deny still wins"
        );
        assert_eq!(p.network.egress.http[1].verdict, Verdict::Allow);
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
    fn an_approved_tcp_rule_lands_before_the_ask_rule_that_raised_the_prompt() {
        let mut p = Policy::default();
        p.network
            .egress
            .tcp
            .push(TcpEgressRule::new("db.internal:5432", Verdict::Ask));
        p.add_approved_tcp_rule(TcpEgressRule::allow_destination("db.internal:5432"));
        assert_eq!(
            p.network
                .egress
                .tcp
                .iter()
                .map(|r| r.verdict)
                .collect::<Vec<_>>(),
            vec![Verdict::Allow, Verdict::Ask],
            "an ask rule is the only way a raw prompt is raised, so an approval behind it is dead"
        );
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
    fn an_approval_matching_a_rule_the_author_put_behind_the_ask_is_reported_as_unreachable() {
        let mut p = Policy::default();
        p.add_rule(ask_route("api.example.test"));
        p.add_rule(RouteRule::allow_host("api.example.test"));
        assert_eq!(
            p.add_approved_rule(RouteRule::allow_host("api.example.test")),
            Approval::Unreachable("api.example.test".into()),
            "the gate stops at the ask rule, so the copy behind it never fires and the card comes back on the next request"
        );
        assert_eq!(
            p.network
                .egress
                .http
                .iter()
                .map(|r| r.verdict)
                .collect::<Vec<_>>(),
            vec![Verdict::Ask, Verdict::Allow]
        );
    }

    #[test]
    fn an_approval_matching_a_raw_rule_behind_the_raw_ask_is_reported_as_unreachable() {
        let mut p = Policy::default();
        p.network
            .egress
            .tcp
            .push(TcpEgressRule::new("db.internal:5432", Verdict::Ask));
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
    fn an_approved_route_keeps_the_tls_termination_the_ask_rule_asked_for() {
        let mut p = Policy::default();
        p.add_rule(RouteRule {
            tls_terminate: true,
            ..ask_route("api.example.test")
        });
        p.add_approved_rule(RouteRule::allow_host("api.example.test"));
        assert!(
            p.network.egress.http[0].tls_terminate,
            "the allow now decides the destination, so dropping interception here silently stops credential injection for it: {:?}",
            p.network.egress.http
        );
    }

    #[test]
    fn an_approved_route_keeps_the_request_filter_the_ask_rule_carried() {
        let mut p = Policy::default();
        p.add_rule(RouteRule {
            rules: vec![HttpRule {
                method: Some("GET".into()),
                path: None,
            }],
            ..ask_route("api.example.test")
        });
        p.add_approved_rule(RouteRule::allow_host("api.example.test"));
        assert_eq!(
            p.network.egress.http[0].rules,
            vec![HttpRule {
                method: Some("GET".into()),
                path: None,
            }],
            "an unrestricted allow in front would allow every method the author restricted away"
        );
    }

    #[test]
    fn an_approved_route_keeps_the_binary_scope_the_ask_rule_carried() {
        let mut p = Policy::default();
        p.add_rule(RouteRule {
            binaries: Some(vec!["/usr/bin/curl".into()]),
            ..ask_route("api.example.test")
        });
        p.add_approved_rule(RouteRule::allow_host("api.example.test"));
        assert_eq!(
            p.network.egress.http[0].binaries.as_deref(),
            Some(&["/usr/bin/curl".to_string()][..]),
            "approving the caller that asked is not consent to open the destination to every other caller"
        );
    }

    #[test]
    fn an_approved_route_under_a_wildcard_ask_grants_only_the_host_on_the_card() {
        let mut p = Policy::default();
        p.add_rule(RouteRule {
            tls_terminate: true,
            ..ask_route("*.example.test")
        });
        p.add_approved_rule(RouteRule::allow_host("api.example.test"));
        assert_eq!(
            p.network
                .egress
                .http
                .iter()
                .map(|r| (r.match_pattern.as_str(), r.verdict, r.tls_terminate))
                .collect::<Vec<_>>(),
            vec![
                ("api.example.test", Verdict::Allow, true),
                ("*.example.test", Verdict::Ask, true)
            ],
            "the card named one host; taking the wildcard's pattern too would grant every other subdomain without ever asking, and the ask rule exists to keep asking about them"
        );
    }

    #[test]
    fn an_approved_deny_does_not_take_on_the_ask_rules_tls_termination() {
        let mut p = Policy::default();
        p.add_rule(RouteRule {
            tls_terminate: true,
            ..ask_route("api.example.test")
        });
        p.add_approved_rule(RouteRule::deny_host("api.example.test"));
        assert_eq!(
            (
                p.network.egress.http[0].verdict,
                p.network.egress.http[0].tls_terminate
            ),
            (Verdict::Deny, false),
            "a deny blocks the request before there is anything to intercept"
        );
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
        p.add_rule(ask_route("api.example.test"));
        assert_eq!(
            p.add_approved_rule(RouteRule::allow_host("api.example.test")),
            Approval::Stands
        );
        assert_eq!(
            p.add_approved_rule(RouteRule::deny_host("api.example.test")),
            Approval::Shadowed("api.example.test".into()),
            "one ask rule raises one card per request, so the second answer arrives after the first has already written the rule that now decides"
        );
        assert_eq!(
            p.network
                .egress
                .http
                .iter()
                .map(|r| r.verdict)
                .collect::<Vec<_>>(),
            vec![Verdict::Allow, Verdict::Ask]
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
    fn an_approved_tcp_rule_keeps_the_binary_scope_the_raw_ask_carried() {
        let mut p = Policy::default();
        p.network.egress.tcp.push(TcpEgressRule {
            binaries: Some(vec!["/usr/bin/psql".into()]),
            ..TcpEgressRule::new("db.internal:5432", Verdict::Ask)
        });
        p.add_approved_tcp_rule(TcpEgressRule::allow_destination("db.internal:5432"));
        assert_eq!(
            p.network.egress.tcp[0].binaries.as_deref(),
            Some(&["/usr/bin/psql".to_string()][..]),
            "an opaque splice for one binary is not consent to splice it for every caller"
        );
    }

    #[test]
    fn an_approved_tcp_rule_under_a_range_ask_splices_only_the_address_on_the_card() {
        let mut p = Policy::default();
        p.network
            .egress
            .tcp
            .push(TcpEgressRule::new("10.0.0.0/24:5432", Verdict::Ask));
        p.add_approved_tcp_rule(TcpEgressRule::allow_destination("10.0.0.5:5432"));
        assert_eq!(
            p.network
                .egress
                .tcp
                .iter()
                .map(|r| (r.match_pattern.as_str(), r.verdict))
                .collect::<Vec<_>>(),
            vec![
                ("10.0.0.5:5432", Verdict::Allow),
                ("10.0.0.0/24:5432", Verdict::Ask)
            ],
            "taking the range's pattern would open an opaque, unaudited splice to 256 addresses off one approval for one of them"
        );
    }

    #[test]
    fn add_rule_still_appends_so_a_hand_written_order_is_preserved() {
        let mut p = Policy::default();
        p.add_rule(ask_route("api.example.test"));
        p.add_rule(RouteRule::allow_host("api.example.test"));
        assert_eq!(
            p.network
                .egress
                .http
                .iter()
                .map(|r| r.verdict)
                .collect::<Vec<_>>(),
            vec![Verdict::Ask, Verdict::Allow],
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
    fn load_or_default_rejects_an_upstream_default_transport() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let yaml = "\
network:
  defaultVerdict: ask
  defaultTransport: upstream
";
        fs::write(&path, yaml).unwrap();
        let err = Policy::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string()
                .contains("upstream transport isn't supported in the local sandbox"),
            "got: {err}"
        );
    }

    #[test]
    fn load_or_default_rejects_an_upstream_route_transport() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let yaml = "\
network:
  egress:
    http:
      - match: api.example.test
        verdict: allow
        transport: upstream
  defaultVerdict: ask
";
        fs::write(&path, yaml).unwrap();
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
        fs::write(&path, "network: not-a-map\n").unwrap();
        let err = Policy::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn save_atomic_writes_file_readable_by_load_or_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("api.linear.app"));

        p.save_atomic(&path).unwrap();
        let reloaded = Policy::load_or_default(&path).unwrap();
        assert_eq!(p, reloaded);
    }

    #[test]
    fn save_atomic_creates_parent_directory() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/dir/lns-policy.yaml");
        Policy::default().save_atomic(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_atomic_leaves_no_tmp_file_on_success() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        Policy::default().save_atomic(&path).unwrap();
        let tmp = path.with_extension("yaml.tmp");
        assert!(!tmp.exists(), "tmp file should be renamed away");
    }

    #[test]
    fn file_policy_store_save_writes_yaml_readable_by_load_or_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let store = FilePolicyStore::new(path.clone());

        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("api.linear.app"));
        store.save(&p).unwrap();

        let reloaded = Policy::load_or_default(&path).unwrap();
        assert_eq!(reloaded, p);
    }

    #[test]
    fn file_policy_store_save_to_unwritable_parent_surfaces_error() {
        let dir = TempDir::new().unwrap();
        let unwritable = dir.path().join("file-not-a-dir");
        fs::write(&unwritable, b"").unwrap();
        let path = unwritable.join("nested/lns-policy.yaml");
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
    fn legacy_network_only_yaml_parses_with_empty_connectors() {
        let yaml = "\
network:
  egress:
    http: []
  defaultVerdict: ask
  defaultTransport: direct
";
        let p: Policy = serde_yaml::from_str(yaml).unwrap();
        assert!(p.connectors.is_empty());
    }

    #[test]
    fn default_policy_omits_the_connectors_key() {
        let yaml = serde_yaml::to_string(&Policy::default()).unwrap();
        assert!(
            !yaml.contains("connectors"),
            "an empty connectors list must not clutter the shareable file:\n{yaml}"
        );
    }

    #[test]
    fn policy_with_connectors_yaml_roundtrip_is_lossless() {
        let mut p = Policy::default();
        p.connect("gitlab");
        p.connect("acme");
        let yaml = serde_yaml::to_string(&p).unwrap();
        assert!(yaml.contains("connectors"), "got:\n{yaml}");
        let parsed: Policy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(p, parsed);
    }

    #[test]
    fn a_legacy_policy_carrying_the_removed_credentials_section_still_loads() {
        let yaml = "\
network:
  egress:
    http: []
  defaultVerdict: ask
  defaultTransport: direct
credentials:
  customProviders:
    - id: acme
      envVar: ACME_API_KEY
      placeholder: acme_LNSPLACEHOLDER0000000000000000000000
      injections:
        - kind: bearer_header
          domain: api.acme.corp
";
        let p: Policy = serde_yaml::from_str(yaml).unwrap();
        assert!(
            p.connectors.is_empty(),
            "the now-unknown credentials section is ignored, not an error"
        );
    }

    #[test]
    fn connect_adds_an_connector_id() {
        let mut p = Policy::default();
        p.connect("github");
        assert_eq!(p.connectors, ["github"]);
    }

    #[test]
    fn connect_is_idempotent_and_does_not_duplicate() {
        let mut p = Policy::default();
        p.connect("github");
        p.connect("github");
        assert_eq!(p.connectors, ["github"]);
    }

    #[test]
    fn disconnect_removes_an_applied_connector_and_reports_true() {
        let mut p = Policy::default();
        p.connect("github");
        p.connect("gitlab");
        assert!(p.disconnect("github"));
        assert_eq!(p.connectors, ["gitlab"]);
    }

    #[test]
    fn disconnect_reports_false_when_the_id_is_not_applied() {
        let mut p = Policy::default();
        p.connect("gitlab");
        assert!(!p.disconnect("github"));
        assert_eq!(p.connectors, ["gitlab"]);
    }
}
