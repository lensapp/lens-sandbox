use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

use lns_policy::{Egress, HttpRule, NetworkPolicy, Policy, Scheme, Transport, Verdict};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostFrame {
    Policy(PolicyMessage),
    RequestDecision(RequestDecision),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GuestFrame {
    RequestPending(RequestPending),
    #[serde(other)]
    Other,
}

/// The verdict the guest gate accepts. `ask` is one no document may write — `lns_policy::Verdict` stays two-valued so it cannot reach `lns-local-mixin.yaml` — but the gate has always taken it, and it is how a served destination is held (sandbox-spec §3.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WireVerdict {
    Allow,
    Deny,
    Ask,
}

impl From<Verdict> for WireVerdict {
    fn from(verdict: Verdict) -> Self {
        match verdict {
            Verdict::Allow => Self::Allow,
            Verdict::Deny => Self::Deny,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireRouteRule {
    #[serde(rename = "match")]
    pub match_pattern: String,
    pub verdict: WireVerdict,
    #[serde(default)]
    pub transport: Transport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<Scheme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tls_terminate: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<HttpRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binaries: Option<Vec<String>>,
}

impl From<&lns_policy::RouteRule> for WireRouteRule {
    fn from(rule: &lns_policy::RouteRule) -> Self {
        Self {
            match_pattern: rule.match_pattern.clone(),
            verdict: rule.verdict.into(),
            transport: rule.transport,
            scheme: rule.scheme,
            description: rule.description.clone(),
            tls_terminate: rule.tls_terminate,
            rules: rule.rules.clone(),
            binaries: rule.binaries.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireTcpEgressRule {
    #[serde(rename = "match")]
    pub match_pattern: String,
    pub verdict: WireVerdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binaries: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl From<&lns_policy::TcpEgressRule> for WireTcpEgressRule {
    fn from(rule: &lns_policy::TcpEgressRule) -> Self {
        Self {
            match_pattern: rule.match_pattern.clone(),
            verdict: rule.verdict.into(),
            binaries: rule.binaries.clone(),
            description: rule.description.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireEgress {
    pub http: Vec<WireRouteRule>,
    pub tcp: Vec<WireTcpEgressRule>,
}

/// Says the rule is a hold without discarding what the author wrote, since the dialog and the audit show this text for the destination being asked about.
fn held_note(authored: Option<&str>) -> String {
    match authored {
        Some(authored) => format!("held for a connector offer: {authored}"),
        None => "held for a connector offer".to_string(),
    }
}

/// Hold every destination a served pattern shares with something the run allows, by inserting that allow narrowed to the shared destinations ahead of itself (§3.2.1).
fn held_for_offers(egress: &Egress, serves: &[String]) -> WireEgress {
    let mut http = Vec::new();
    for rule in &egress.http {
        if rule.verdict == Verdict::Allow {
            for pattern in serves {
                if let Some(shared) =
                    lns_policy::matching::intersection(pattern, &rule.match_pattern)
                {
                    let mut asked = WireRouteRule::from(rule);
                    asked.match_pattern = shared;
                    asked.verdict = WireVerdict::Ask;
                    asked.description = Some(held_note(rule.description.as_deref()));
                    http.push(asked);
                }
            }
        }
        http.push(WireRouteRule::from(rule));
    }
    let mut tcp = Vec::new();
    for rule in &egress.tcp {
        if rule.verdict == Verdict::Allow {
            for pattern in serves {
                if let Some(shared) =
                    lns_policy::matching::intersection(pattern, &rule.match_pattern)
                {
                    let mut asked = WireTcpEgressRule::from(rule);
                    asked.match_pattern = shared;
                    asked.verdict = WireVerdict::Ask;
                    asked.description = Some(held_note(rule.description.as_deref()));
                    tcp.push(asked);
                }
            }
        }
        tcp.push(WireTcpEgressRule::from(rule));
    }
    WireEgress { http, tcp }
}

/// The `network` section as the guest gate requires it: both defaults left the policy file, but a guest missing `defaultVerdict` fails every destination closed, so it is derived from the rules here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireNetwork {
    pub egress: WireEgress,
    pub default_verdict: WireDefaultVerdict,
    pub default_transport: Transport,
}

impl WireNetwork {
    pub fn seeded(network: NetworkPolicy) -> Self {
        Self::seeded_holding(network, &[])
    }

    /// [`Self::seeded`], plus the destinations `serves` covers held for an offer.
    pub fn seeded_holding(network: NetworkPolicy, serves: &[String]) -> Self {
        let default_verdict = if network.is_closed() {
            WireDefaultVerdict::Deny
        } else {
            WireDefaultVerdict::Ask
        };
        Self {
            egress: held_for_offers(&network.egress, serves),
            default_verdict,
            default_transport: Transport::Direct,
        }
    }
}

/// The default a policy can arrive at is two-valued: no document writes `ask`, and a closed policy denies. A served destination is held by a rule instead (§3.2.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WireDefaultVerdict {
    #[default]
    Ask,
    Deny,
}

/// One credential as the boundary arms it: the placeholder the workload sees, and the real value substituted per domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireCredential {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_var: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub injections: Vec<WireInjection>,
}

/// `UriPlaceholder` substitutes the parent credential's placeholder inside matching outbound URLs, so unlike `Header` it carries no header of its own.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "injectionType",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WireInjection {
    Header {
        domain: String,
        header: String,
        value: String,
    },
    UriPlaceholder {
        domain: String,
        value: String,
    },
}

impl WireInjection {
    /// The value the boundary substitutes, for tests and for the arming check — never for display.
    pub fn value(&self) -> &str {
        match self {
            Self::Header { value, .. } | Self::UriPlaceholder { value, .. } => value,
        }
    }
}

/// Hand-written so no `log::debug!` of a policy frame can put a live credential on the trace stream; an unarmed injection says so, because that is the state worth seeing.
impl std::fmt::Debug for WireInjection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (kind, domain, armed) = match self {
            Self::Header { domain, value, .. } => ("Header", domain, !value.is_empty()),
            Self::UriPlaceholder { domain, value } => ("UriPlaceholder", domain, !value.is_empty()),
        };
        write!(
            f,
            "{kind} {{ domain: {domain:?}, value: {} }}",
            if armed { "<redacted>" } else { "<unarmed>" }
        )
    }
}

/// One file a granted method writes into the running guest, in the shape `lens-sandbox-core` reads.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireFile {
    pub path: String,
    #[serde(flatten)]
    pub content: WireFileContent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    /// Absent is core's own default of `workload`, so only a document withholding a file from it says anything here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<WireFileOwner>,
}

/// An entry carrying both spellings, or neither, is one core refuses, so neither state is representable here.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WireFileContent {
    Content(String),
    ContentB64(String),
}

/// The content itself never renders, for the reason [`WireFile`]'s own `Debug` does not print it.
impl std::fmt::Debug for WireFileContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let spelling = match self {
            Self::Content(_) => "Content",
            Self::ContentB64(_) => "ContentB64",
        };
        write!(f, "{spelling}(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireFileOwner {
    Root,
}

impl WireFileOwner {
    /// `None` for the workload, which core defaults to, so only a document withholding a file says anything.
    fn declared_by(owner: lns_artifact::sandbox::FilesetOwner) -> Option<Self> {
        match owner {
            lns_artifact::sandbox::FilesetOwner::Workload => None,
            lns_artifact::sandbox::FilesetOwner::Root => Some(Self::Root),
        }
    }
}

impl WireFile {
    pub fn text(path: &str, content: &str, mode: Option<u32>) -> Self {
        Self {
            path: path.to_string(),
            content: WireFileContent::Content(content.to_string()),
            mode,
            owner: None,
        }
    }

    /// A packed file is arbitrary bytes, which text is not a shape for.
    pub fn bytes(path: &str, content: &[u8], mode: Option<u32>) -> Self {
        Self {
            path: path.to_string(),
            content: WireFileContent::ContentB64(BASE64.encode(content)),
            mode,
            owner: None,
        }
    }

    pub fn owned_by(mut self, owner: lns_artifact::sandbox::FilesetOwner) -> Self {
        self.owner = WireFileOwner::declared_by(owner);
        self
    }
}

/// §3.2.5 requires a fileset to carry a placeholder rather than a value, but a document that breaks that rule must not reach the trace stream through us.
impl std::fmt::Debug for WireFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WireFile")
            .field("path", &self.path)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

/// Outbound `policy` payload; the receiver tolerates extra fields, so we send only `network`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<WireNetwork>,
    /// The guest replaces every section a frame carries, so a grant's credentials ride on every later frame or the next one retracts them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<Vec<WireCredential>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<WireFile>>,
}

/// What a granted method contributes to the running policy, kept together because the guest replaces each section it receives (§3.3.2 source 4).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GrantedPayload {
    pub egress: Policy,
    pub credentials: Vec<WireCredential>,
    pub env: BTreeMap<String, String>,
    pub files: Vec<WireFile>,
}

impl GrantedPayload {
    /// Every grant this project holds as one layer. A run may grant more than one connector, and the guest replaces each section it receives — so a frame carrying one grant retracts the rest.
    ///
    /// Ordered by connector name: install refuses two connectors that serve the same destination, so their rules cannot contradict, and a stable order keeps the published table the same between runs.
    pub fn combined(grants: &BTreeMap<String, Self>) -> Option<Self> {
        if grants.is_empty() {
            return None;
        }
        let mut combined = Self::default();
        for granted in grants.values() {
            combined
                .egress
                .network
                .egress
                .http
                .extend(granted.egress.network.egress.http.iter().cloned());
            combined
                .egress
                .network
                .egress
                .tcp
                .extend(granted.egress.network.egress.tcp.iter().cloned());
            combined.credentials.extend(granted.credentials.clone());
            combined.env.extend(granted.env.clone());
            combined.files.extend(granted.files.clone());
        }
        Some(combined)
    }
}

impl PolicyMessage {
    /// The one place a published policy is shaped, because the guest replaces every map it carries on each apply — a section left out here retracts it.
    pub fn seeded(policy: Policy) -> Self {
        Self::seeded_holding(policy, &[])
    }

    /// The frame a run with a granted connector sends: the same table, plus what the method supplies.
    pub fn granting(policy: Policy, serves: &[String], granted: &GrantedPayload) -> Self {
        Self {
            credentials: Some(granted.credentials.clone()).filter(|c| !c.is_empty()),
            env: Some(granted.env.clone()).filter(|e| !e.is_empty()),
            files: Some(granted.files.clone()).filter(|f| !f.is_empty()),
            ..Self::seeded_holding(policy, serves)
        }
    }

    /// [`Self::seeded`], plus the destinations `serves` covers held for an offer (sandbox-spec §3.2.1).
    pub fn seeded_holding(policy: Policy, serves: &[String]) -> Self {
        Self {
            network: Some(WireNetwork::seeded_holding(policy.network, serves)),
            credentials: None,
            env: None,
            files: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestPending {
    pub id: String,
    pub host: String,
    pub action: String,
    pub reason: String,
    /// Absent on a guest that predates the field; either way the reading that grants less is the safe default.
    #[serde(default)]
    pub treatment: Treatment,
}

/// What approving a held request actually permits: `action` renders identically either way, but a raw splice is opaque to the proxy — no HTTP rules and no per-request audit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Treatment {
    Raw,
    /// Also how a treatment this lns doesn't know reads, since failing the frame would drop the card and hang the workload.
    #[default]
    #[serde(other)]
    Inspected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestDecision {
    pub id: String,
    pub decision: Decision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    AllowAlways,
    AllowOnce,
    DenyAlways,
    DenyOnce,
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::{NetworkPolicy, RouteRule, Transport, Verdict};
    use serde_json::json;

    #[test]
    fn a_granted_fileset_rides_the_frame_in_the_shape_the_guest_reads() {
        let granted = GrantedPayload {
            files: vec![
                WireFile::text("~/.some-provider/config.json", "{}", None),
                WireFile::bytes("~/.some-provider/seal.bin", &[0, 1], Some(0o600)),
            ],
            ..GrantedPayload::default()
        };
        let frame = PolicyMessage::granting(Policy::default(), &[], &granted);
        let v = serde_json::to_value(&frame).unwrap();
        assert_eq!(v["files"][0]["path"], "~/.some-provider/config.json");
        assert_eq!(v["files"][0]["content"], "{}");
        assert!(
            v["files"][0].get("contentB64").is_none(),
            "core refuses an entry that sets both content and contentB64"
        );
        assert_eq!(
            v["files"][1]["contentB64"], "AAE=",
            "a packed file is arbitrary bytes, so text is not a shape it always has"
        );
        assert_eq!(v["files"][1]["mode"], 384);
    }

    #[test]
    fn a_grant_without_a_fileset_carries_no_files_section() {
        let frame = PolicyMessage::granting(Policy::default(), &[], &GrantedPayload::default());
        let v = serde_json::to_value(&frame).unwrap();
        assert!(
            v.get("files").is_none(),
            "an empty section is not the same as no section: core deletes what a frame it receives does not carry"
        );
    }

    #[test]
    fn every_grants_files_ride_one_frame() {
        let grants = BTreeMap::from([
            (
                "docs".to_string(),
                GrantedPayload {
                    files: vec![WireFile::text("~/.docs/config.json", "{}", None)],
                    ..GrantedPayload::default()
                },
            ),
            (
                "some-provider".to_string(),
                GrantedPayload {
                    files: vec![WireFile::text("~/.some-provider/config.json", "{}", None)],
                    ..GrantedPayload::default()
                },
            ),
        ]);
        let combined =
            GrantedPayload::combined(&grants).expect("two grants combine into one layer");
        assert_eq!(
            combined
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["~/.docs/config.json", "~/.some-provider/config.json"],
            "the guest replaces the whole section, so a frame carrying one grant's files would retract the rest"
        );
    }

    #[test]
    fn a_wire_file_never_prints_its_content() {
        let rendered = format!(
            "{:?}",
            WireFile::text("~/.some-provider/credentials.json", "sk-live-real", None)
        );
        assert!(
            rendered.contains("~/.some-provider/credentials.json") && !rendered.contains("sk-live"),
            "a fileset carries a placeholder by rule, but a document that breaks the rule must not put its content on the trace stream; got: {rendered}"
        );
    }

    #[test]
    fn neither_spelling_of_file_content_renders_what_it_holds() {
        for content in [
            WireFileContent::Content("sk-live-real".to_string()),
            WireFileContent::ContentB64(BASE64.encode(b"sk-live-real")),
        ] {
            let rendered = format!("{content:?}");
            assert!(
                rendered.contains("<redacted>") && !rendered.contains("sk-live"),
                "base64 is an encoding, not a secret, so the encoded spelling must redact for the same reason the plain one does; got: {rendered}"
            );
        }
    }

    #[test]
    fn policy_frame_serializes_with_type_discriminator() {
        let frame = HostFrame::Policy(PolicyMessage {
            network: Some(WireNetwork::seeded(NetworkPolicy::default())),
            ..PolicyMessage::default()
        });
        let v: serde_json::Value = serde_json::to_value(&frame).unwrap();
        assert_eq!(v["type"], "policy");
        assert_eq!(v["network"]["defaultVerdict"], "ask");
        assert_eq!(
            v["network"]["defaultTransport"], "direct",
            "lens-sandbox-core fail-closes a non-deny verdict to deny when defaultTransport is missing"
        );
        assert_eq!(v["network"]["egress"]["http"], json!([]));
    }

    #[test]
    fn a_closed_policy_tells_the_guest_not_to_ask_either() {
        // A catch-all deny decides everything the tables are consulted for, but a
        // connection the guest cannot classify reads the default instead — so a
        // locked-down policy has to say so there too, or it would still raise cards
        // for the traffic it exists to refuse.
        let mut closed = NetworkPolicy::default();
        closed.egress.http.push(RouteRule::deny_host("*"));
        let v = serde_json::to_value(WireNetwork::seeded(closed)).unwrap();
        assert_eq!(v["defaultVerdict"], "deny");

        let open = serde_json::to_value(WireNetwork::seeded(NetworkPolicy::default())).unwrap();
        assert_eq!(
            open["defaultVerdict"], "ask",
            "a policy that decides nothing must keep asking"
        );
    }

    #[test]
    fn the_policy_frame_publishes_one_egress_table_and_not_the_deprecated_list() {
        let frame = HostFrame::Policy(PolicyMessage {
            network: Some(WireNetwork::seeded(NetworkPolicy::default())),
            ..PolicyMessage::default()
        });
        let v: serde_json::Value = serde_json::to_value(&frame).unwrap();
        let network = v["network"].as_object().expect("network is an object");
        assert!(
            !network.contains_key("allowedRoutes"),
            "an egress block supersedes allowedRoutes in the guest, so a route sent in both is a route sent in neither: {network:?}"
        );
        let egress = network["egress"]
            .as_object()
            .expect("a null or scalar egress makes the guest force-deny every destination");
        assert_eq!(
            egress.keys().collect::<Vec<_>>(),
            ["http", "tcp"],
            "core denies unknown fields under egress, so one stray key fails the whole policy closed: {egress:?}"
        );
    }

    #[test]
    fn the_policy_frame_publishes_a_tcp_rule_without_a_transport() {
        let mut net = NetworkPolicy::default();
        net.egress
            .tcp
            .push(lns_policy::TcpEgressRule::allow_destination(
                "db.internal:5432",
            ));
        let frame = HostFrame::Policy(PolicyMessage {
            network: Some(WireNetwork::seeded(net)),
            ..PolicyMessage::default()
        });
        let v: serde_json::Value = serde_json::to_value(&frame).unwrap();
        let rule = v["network"]["egress"]["tcp"][0]
            .as_object()
            .expect("the tcp table carries the rule");
        assert_eq!(rule["match"], "db.internal:5432");
        assert!(
            !rule.contains_key("transport"),
            "raw egress is always direct: core's TcpEgressRule has no transport and would ignore ours, so publishing one would put a routing choice on the wire that nothing honors: {rule:?}"
        );
    }

    #[test]
    fn every_route_in_the_frame_carries_its_transport_even_when_direct() {
        let mut net = NetworkPolicy::default();
        net.egress.http.push(RouteRule::allow_host("10.0.0.0/8"));
        let frame = HostFrame::Policy(PolicyMessage {
            network: Some(WireNetwork::seeded(net)),
            ..PolicyMessage::default()
        });
        let v: serde_json::Value = serde_json::to_value(&frame).unwrap();
        assert_eq!(
            v["network"]["egress"]["http"][0]["transport"], "direct",
            "core's route schema has no transport default — one missing transport fails the whole route parse and clears every route"
        );
    }

    #[test]
    fn policy_frame_round_trips() {
        let mut net = NetworkPolicy::default();
        net.egress
            .http
            .push(RouteRule::allow_host("api.linear.app"));
        let frame = HostFrame::Policy(PolicyMessage {
            network: Some(WireNetwork::seeded(net)),
            ..PolicyMessage::default()
        });

        let s = serde_json::to_string(&frame).unwrap();
        let parsed: HostFrame = serde_json::from_str(&s).unwrap();
        assert_eq!(frame, parsed);
    }

    #[test]
    fn request_decision_serializes_decision_in_camel_case() {
        let frame = HostFrame::RequestDecision(RequestDecision {
            id: "req-1".into(),
            decision: Decision::AllowAlways,
        });
        let v: serde_json::Value = serde_json::to_value(&frame).unwrap();
        assert_eq!(v["type"], "request_decision");
        assert_eq!(v["id"], "req-1");
        assert_eq!(v["decision"], "allow_always");
    }

    #[test]
    fn every_decision_variant_serializes_to_snake_case_token_matching_in_sandbox_proxy() {
        for (variant, expected) in [
            (Decision::AllowOnce, "allow_once"),
            (Decision::AllowAlways, "allow_always"),
            (Decision::DenyOnce, "deny_once"),
            (Decision::DenyAlways, "deny_always"),
            (Decision::Timeout, "timeout"),
        ] {
            let s = serde_json::to_string(&variant).unwrap();
            assert_eq!(s.trim_matches('"'), expected);
        }
    }

    #[test]
    fn request_pending_parses_from_gate_emitted_json() {
        let raw = r#"{
            "type": "request_pending",
            "id": "req-42",
            "host": "api.linear.app",
            "action": "CONNECT api.linear.app:443",
            "reason": "policy-ambiguous",
            "treatment": "inspected"
        }"#;
        let parsed: GuestFrame = serde_json::from_str(raw).unwrap();
        assert!(
            matches!(&parsed, GuestFrame::RequestPending(rp)
                if rp.id == "req-42"
                    && rp.host == "api.linear.app"
                    && rp.action == "CONNECT api.linear.app:443"
                    && rp.reason == "policy-ambiguous"
                    && rp.treatment == Treatment::Inspected),
            "got {parsed:?}"
        );
    }

    #[test]
    fn a_raw_splice_prompt_parses_with_its_treatment() {
        let raw = r#"{
            "type": "request_pending",
            "id": "req-43",
            "host": "db.internal:5432",
            "action": "CONNECT db.internal:5432",
            "reason": "policy-ambiguous",
            "treatment": "raw"
        }"#;
        let parsed: GuestFrame = serde_json::from_str(raw).unwrap();
        assert!(
            matches!(&parsed, GuestFrame::RequestPending(rp) if rp.treatment == Treatment::Raw),
            "a raw splice is the consequential approval; reading it as inspected would persist the wrong table: {parsed:?}"
        );
    }

    #[test]
    fn a_prompt_from_a_guest_that_predates_treatment_reads_as_inspected() {
        let raw = r#"{
            "type": "request_pending",
            "id": "req-44",
            "host": "api.linear.app",
            "action": "CONNECT api.linear.app:443",
            "reason": "policy-ambiguous"
        }"#;
        let parsed: GuestFrame = serde_json::from_str(raw).unwrap();
        assert!(
            matches!(&parsed, GuestFrame::RequestPending(rp) if rp.treatment == Treatment::Inspected),
            "a missing treatment must not fail the frame, and inspected is the answer that grants less: {parsed:?}"
        );
    }

    #[test]
    fn a_prompt_carrying_a_treatment_this_lns_does_not_know_still_raises_a_card() {
        let raw = r#"{
            "type": "request_pending",
            "id": "req-45",
            "host": "api.linear.app",
            "action": "CONNECT api.linear.app:443",
            "reason": "policy-ambiguous",
            "treatment": "some-later-treatment"
        }"#;
        let parsed: GuestFrame = serde_json::from_str(raw).unwrap();
        assert!(
            matches!(&parsed, GuestFrame::RequestPending(rp) if rp.treatment == Treatment::Inspected),
            "failing the frame would drop the card entirely and hang the workload until the approval times out: {parsed:?}"
        );
    }

    #[test]
    fn unrecognised_guest_frame_lands_in_other_not_error() {
        let raw = r#"{"type":"audit_event","payload":{"x":1}}"#;
        let parsed: GuestFrame = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed, GuestFrame::Other);
    }

    #[test]
    fn host_frame_text_envelope_matches_lens_sandbox_core_dispatch() {
        let policy_s = serde_json::to_value(HostFrame::Policy(PolicyMessage::default())).unwrap();
        let decision_s = serde_json::to_value(HostFrame::RequestDecision(RequestDecision {
            id: "x".into(),
            decision: Decision::DenyOnce,
        }))
        .unwrap();
        assert_eq!(policy_s["type"], "policy");
        assert_eq!(decision_s["type"], "request_decision");
    }

    #[test]
    fn empty_policy_frame_serializes_with_only_type_field() {
        let frame = HostFrame::Policy(PolicyMessage::default());
        let s = serde_json::to_string(&frame).unwrap();
        assert_eq!(s, r#"{"type":"policy"}"#);
    }

    #[test]
    fn network_policy_in_frame_serializes_route_rule_with_match_key() {
        let mut net = NetworkPolicy::default();
        net.egress.http.push(RouteRule {
            match_pattern: "api.linear.app".into(),
            verdict: Verdict::Allow,
            transport: Transport::Upstream,
            scheme: None,
            description: None,
            tls_terminate: false,
            rules: Vec::new(),
            binaries: None,
        });
        let frame = HostFrame::Policy(PolicyMessage {
            network: Some(WireNetwork::seeded(net)),
            ..PolicyMessage::default()
        });
        let s = serde_json::to_string(&frame).unwrap();
        assert!(s.contains(r#""match":"api.linear.app""#), "got: {s}");
        assert!(!s.contains("matchPattern"), "rust ident leaked: {s}");
    }
    fn allowing(pattern: &str) -> lns_policy::RouteRule {
        lns_policy::RouteRule::allow_host(pattern)
    }

    fn denying(pattern: &str) -> lns_policy::RouteRule {
        let mut rule = lns_policy::RouteRule::allow_host(pattern);
        rule.verdict = Verdict::Deny;
        rule
    }

    fn http_of(egress: Egress, serves: &[&str]) -> Vec<(String, WireVerdict)> {
        let serves: Vec<String> = serves.iter().map(|s| (*s).to_string()).collect();
        WireNetwork::seeded_holding(NetworkPolicy { egress }, &serves)
            .egress
            .http
            .into_iter()
            .map(|rule| (rule.match_pattern, rule.verdict))
            .collect()
    }

    #[test]
    fn a_served_destination_the_run_allows_is_held_ahead_of_that_allow() {
        // §3.2.1: it asks whatever the run's own egress allowed — a request that arrives unauthenticated fails as surely as one that is blocked.
        let egress = Egress {
            http: vec![allowing("api.some-provider.example")],
            tcp: Vec::new(),
        };
        assert_eq!(
            http_of(egress, &["api.some-provider.example"]),
            [
                ("api.some-provider.example".to_string(), WireVerdict::Ask),
                ("api.some-provider.example".to_string(), WireVerdict::Allow),
            ]
        );
    }

    #[test]
    fn a_served_destination_the_run_denies_is_not_held() {
        // §3.2.1: a deny is an explicit refusal, so it silences the offer rather than being asked about.
        let egress = Egress {
            http: vec![denying("api.some-provider.example")],
            tcp: Vec::new(),
        };
        assert_eq!(
            http_of(egress, &["api.some-provider.example"]),
            [("api.some-provider.example".to_string(), WireVerdict::Deny)]
        );
    }

    #[test]
    fn an_allow_a_deny_already_shadows_stays_shadowed() {
        // The user's own order is preserved: `allow api` then `deny *` means api is allowed and the rest denied, and inserting ahead of the allow cannot change which of the two a destination reaches first.
        let egress = Egress {
            http: vec![
                allowing("api.some-provider.example"),
                denying("*.some-provider.example"),
            ],
            tcp: Vec::new(),
        };
        assert_eq!(
            http_of(egress, &["*.some-provider.example"]),
            [
                ("api.some-provider.example".to_string(), WireVerdict::Ask),
                ("api.some-provider.example".to_string(), WireVerdict::Allow),
                ("*.some-provider.example".to_string(), WireVerdict::Deny),
            ],
            "the held rule is a subset of the allow it precedes, so nothing that reached the deny first now reaches an ask"
        );
    }

    #[test]
    fn a_destination_the_run_never_allowed_is_not_held_at_all() {
        // An open policy asks by falling through to the default, and a closed one is denied by its catch-all; either way a hold would add nothing.
        let egress = Egress {
            http: vec![allowing("api.other-provider.example")],
            tcp: Vec::new(),
        };
        assert_eq!(
            http_of(egress, &["api.some-provider.example"]),
            [("api.other-provider.example".to_string(), WireVerdict::Allow)]
        );
    }

    #[test]
    fn a_broad_allow_is_held_only_where_it_meets_the_served_pattern() {
        // Narrowing to the intersection is what keeps the hold from capturing traffic the connector does not serve.
        let egress = Egress {
            http: vec![allowing("*.example.test")],
            tcp: Vec::new(),
        };
        assert_eq!(
            http_of(egress, &["api.example.test"]),
            [
                ("api.example.test".to_string(), WireVerdict::Ask),
                ("*.example.test".to_string(), WireVerdict::Allow),
            ]
        );
    }

    #[test]
    fn a_held_rule_keeps_the_scope_of_the_rule_it_precedes() {
        // Core fails an `ask` with no transport closed, and a hold that dropped `binaries` or `scheme` would capture callers and schemes the allow excluded.
        let mut rule = allowing("api.some-provider.example");
        rule.binaries = Some(vec!["/usr/bin/curl".to_string()]);
        rule.scheme = Some(Scheme::Https);
        rule.transport = Transport::Direct;
        let held = WireNetwork::seeded_holding(
            NetworkPolicy {
                egress: Egress {
                    http: vec![rule],
                    tcp: Vec::new(),
                },
            },
            &["api.some-provider.example".to_string()],
        );
        let asked = &held.egress.http[0];
        assert_eq!(asked.verdict, WireVerdict::Ask);
        assert_eq!(
            asked.binaries.as_deref(),
            Some(&["/usr/bin/curl".to_string()][..])
        );
        assert_eq!(asked.scheme, Some(Scheme::Https));
        assert_eq!(asked.transport, Transport::Direct);
    }

    #[test]
    fn a_served_raw_destination_is_held_in_the_tcp_table_too() {
        // `serves` is one list and the transport belongs to the method, so a raw-stream service is held wherever the run allowed it.
        let egress = Egress {
            http: Vec::new(),
            tcp: vec![lns_policy::TcpEgressRule::allow_destination(
                "db.some-provider.example:5432",
            )],
        };
        let wire = WireNetwork::seeded_holding(
            NetworkPolicy { egress },
            &["db.some-provider.example".to_string()],
        );
        assert_eq!(
            wire.egress
                .tcp
                .into_iter()
                .map(|r| (r.match_pattern, r.verdict))
                .collect::<Vec<_>>(),
            [
                (
                    "db.some-provider.example:5432".to_string(),
                    WireVerdict::Ask
                ),
                (
                    "db.some-provider.example:5432".to_string(),
                    WireVerdict::Allow
                ),
            ],
            "a portless served entry meets a ported allow at that port"
        );
    }

    #[test]
    fn a_machine_with_no_offers_leaves_the_table_exactly_as_it_was() {
        let egress = Egress {
            http: vec![allowing("api.some-provider.example"), denying("*")],
            tcp: Vec::new(),
        };
        assert_eq!(
            http_of(egress, &[]),
            [
                ("api.some-provider.example".to_string(), WireVerdict::Allow),
                ("*".to_string(), WireVerdict::Deny),
            ]
        );
    }

    #[test]
    fn a_hold_keeps_the_description_its_rule_carried() {
        // The dialog and the audit show this text for the destination being asked about, so discarding the author's own words loses it exactly where it matters.
        let mut rule = allowing("api.some-provider.example");
        rule.description = Some("the provider API".to_string());
        let held = WireNetwork::seeded_holding(
            NetworkPolicy {
                egress: Egress {
                    http: vec![rule],
                    tcp: Vec::new(),
                },
            },
            &["api.some-provider.example".to_string()],
        );
        let asked = &held.egress.http[0];
        assert_eq!(asked.verdict, WireVerdict::Ask);
        assert_eq!(
            asked.description.as_deref(),
            Some("held for a connector offer: the provider API")
        );
    }

    #[test]
    fn ask_serialises_as_the_verdict_core_reads() {
        let wire = WireNetwork::seeded_holding(
            NetworkPolicy {
                egress: Egress {
                    http: vec![allowing("api.some-provider.example")],
                    tcp: Vec::new(),
                },
            },
            &["api.some-provider.example".to_string()],
        );
        let json = serde_json::to_string(&wire).expect("serialises");
        assert!(json.contains(r#""verdict":"ask""#), "got: {json}");
    }
}
