use anyhow::{Context, Result};
use serde_json::Value;
use std::os::fd::RawFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::Instrument;

use crate::approval_flow::protocol::{
    Credential, GuestFrame, HostFrame, PolicyMessage, WireNetwork,
};
use crate::approval_flow::session::ApprovalSession;
use crate::credential_flow::registry::expand_credentials_for_wire_with_custom;
use crate::credential_flow::session::CredentialSession;
use lns_policy::Policy;

mod adapter;

pub const VSOCK_PORT: u32 = 1024;

const MAX_AUDIT_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_AUDIT_EVENTS_PER_RUN: u32 = 100_000;
const MAX_AUDIT_BYTES_PER_RUN: usize = 64 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct AuditBudget {
    inner: Arc<Mutex<BudgetState>>,
}

struct BudgetState {
    events_remaining: u32,
    bytes_remaining: usize,
}

impl AuditBudget {
    pub(super) fn with_defaults() -> Self {
        Self::new(MAX_AUDIT_EVENTS_PER_RUN, MAX_AUDIT_BYTES_PER_RUN)
    }

    fn new(events_remaining: u32, bytes_remaining: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BudgetState {
                events_remaining,
                bytes_remaining,
            })),
        }
    }

    pub(super) fn try_charge(&self, line_len: usize) -> bool {
        let mut state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if state.events_remaining == 0 || line_len > state.bytes_remaining {
            return false;
        }
        state.events_remaining -= 1;
        state.bytes_remaining -= line_len;
        true
    }
}

fn audit_path(run_id: &str) -> Result<PathBuf> {
    Ok(lns_ipc::audit_log_for_run(run_id)?)
}

pub struct Relay {
    pub url: String,
    pub token: String,
    pub audit_path: PathBuf,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fd_tx: mpsc::UnboundedSender<RawFd>,
}

#[derive(Clone)]
pub(super) struct RunIdentity {
    pub(super) run: String,
    pub(super) microvm: String,
}

pub fn spawn(
    run_id: &str,
    microvm: &str,
    session: Arc<ApprovalSession>,
    credential_session: Arc<CredentialSession>,
    frame_rx: mpsc::UnboundedReceiver<HostFrame>,
    user_env: Vec<String>,
) -> Result<Relay> {
    let token = generate_token();
    let url = format!("vsock://host:{VSOCK_PORT}/v1/sandbox");
    let audit = audit_path(run_id).context("resolving audit log path")?;
    let (fd_tx, fd_rx) = mpsc::unbounded_channel::<RawFd>();

    let token_clone = token.clone();
    let audit_clone = audit.clone();
    let identity = RunIdentity {
        run: run_id.to_string(),
        microvm: microvm.to_string(),
    };

    let span = tracing::Span::current();
    tokio::spawn(
        async move {
            adapter::accept_loop(
                fd_rx,
                session,
                credential_session,
                frame_rx,
                token_clone,
                audit_clone,
                user_env,
                identity,
            )
            .await;
        }
        .instrument(span),
    );

    Ok(Relay {
        url,
        token,
        audit_path: audit,
        fd_tx,
    })
}

/// `credentials` must be the registry-expanded set, or the supervisor's `on_policy` never seeds the workload env and the credential flow is silently dead.
pub(super) fn initial_policy_frame(policy: &Policy, credentials: Vec<Credential>) -> HostFrame {
    HostFrame::Policy(PolicyMessage {
        network: Some(WireNetwork::seeded(policy.network.clone())),
        credentials: Some(credentials),
    })
}

pub(super) fn current_credentials(session: &CredentialSession) -> Vec<Credential> {
    expand_credentials_for_wire_with_custom(
        &session.current_state(),
        session.custom_providers(),
        &session.armed_ids(),
    )
}

pub(super) struct AuditWriter<'a, L: crate::audit::AuditLog, S: crate::audit::AnchorSink> {
    pub(super) chain: &'a mut lns_ipc::AuditChain,
    pub(super) log: &'a mut L,
    pub(super) anchor: &'a mut S,
    pub(super) run: &'a str,
    pub(super) microvm: &'a str,
}

impl<L: crate::audit::AuditLog, S: crate::audit::AnchorSink> AuditWriter<'_, L, S> {
    fn ctx(&self, clock: &dyn crate::oauth::Clock) -> crate::ocsf_audit::OcsfCtx {
        crate::ocsf_audit::OcsfCtx::at_unix(
            self.run.to_string(),
            self.microvm.to_string(),
            clock.now_unix(),
        )
    }
}

async fn write_chain_line<L: crate::audit::AuditLog, S: crate::audit::AnchorSink>(
    bytes: &[u8],
    writer: &mut AuditWriter<'_, L, S>,
) -> Result<()> {
    let mut line = Vec::with_capacity(bytes.len() + 1);
    line.extend_from_slice(bytes);
    line.push(b'\n');
    writer.log.append_synced(&line).await?;
    if let Some(anchor) = writer.chain.anchor() {
        writer.anchor.write(&anchor)?;
    }
    Ok(())
}

pub(super) async fn write_run_env_event<L: crate::audit::AuditLog, S: crate::audit::AnchorSink>(
    user_env: &[String],
    extra_managed: &[String],
    writer: &mut AuditWriter<'_, L, S>,
    clock: &dyn crate::oauth::Clock,
) -> Result<()> {
    let Some(env) = crate::workload_env::injected_env(user_env, extra_managed) else {
        return Ok(());
    };
    let cx = writer.ctx(clock);
    let bytes = writer
        .chain
        .augment_obj(crate::ocsf_audit::run_env_event(&cx, &env));
    write_chain_line(&bytes, writer).await
}

fn egress_method_and_url(action: &str) -> (&str, &str) {
    let (method, rest) = action.split_once(' ').unwrap_or(("", action));
    if method == "TLS_BRIDGE" {
        let host = rest.split_once(" -> ").map_or(rest, |(host, _addr)| host);
        return (method, host);
    }
    (method, rest)
}

fn guest_egress_to_ocsf(
    cx: &crate::ocsf_audit::OcsfCtx,
    obj: &serde_json::Map<String, Value>,
) -> Option<serde_json::Map<String, Value>> {
    let action = obj.get("action").and_then(Value::as_str)?;
    let (method, url) = egress_method_and_url(action);
    let status_code = obj.get("status_code").and_then(Value::as_u64);
    let result = obj.get("result").and_then(Value::as_str);
    let reason = obj
        .get("metadata")
        .and_then(|m| m.get("reason"))
        .and_then(Value::as_str);
    let mut event =
        crate::ocsf_audit::egress_event(cx, method, url, status_code, result, reason, true);
    for key in ["src_endpoint", "actor"] {
        if let Some(value) = obj.get(key) {
            event.insert(key.to_string(), value.clone());
        }
    }
    Some(event)
}

pub(super) async fn handle_inbound<L: crate::audit::AuditLog, S: crate::audit::AnchorSink>(
    msg: Message,
    session: &Arc<ApprovalSession>,
    credential_session: &Arc<CredentialSession>,
    writer: &mut AuditWriter<'_, L, S>,
    budget: &AuditBudget,
    clock: &dyn crate::oauth::Clock,
) -> Result<bool> {
    let text = match msg {
        Message::Text(text) => text,
        Message::Close(_) => return Ok(true),
        _ => return Ok(false),
    };
    let frame_len = text.len();
    // Dispatch on the parsed top-level `type` so a `request_pending` embedding the literal `"type":"audit_event"` can't be misrouted into the audit chain.
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(text.as_str()) else {
        return Ok(false);
    };
    let ty = obj.get("type").and_then(Value::as_str);
    match ty {
        Some("audit_event") => {
            let cx = writer.ctx(clock);
            let Some(ocsf) = guest_egress_to_ocsf(&cx, &obj) else {
                crate::log::warn!("dropping guest audit_event that is not an egress record");
                return Ok(false);
            };
            if !budget.try_charge(frame_len) {
                crate::log::warn!(
                    "per-run audit-event budget exhausted; dropping guest audit_event"
                );
                return Ok(false);
            }
            let bytes = writer.chain.augment_obj(ocsf);
            write_chain_line(&bytes, writer).await?;
        }
        Some("request_pending") => {
            if let Ok(GuestFrame::RequestPending(req)) =
                serde_json::from_value::<GuestFrame>(Value::Object(obj))
            {
                session.submit_pending(req, Instant::now());
            }
        }
        Some("credential_pending") => {
            if let Ok(GuestFrame::CredentialPending(req)) =
                serde_json::from_value::<GuestFrame>(Value::Object(obj))
            {
                credential_session.submit_pending(req, Instant::now());
            }
        }
        _ => {}
    }
    Ok(false)
}

pub(super) async fn supersede_connection(
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
) {
    if let Some(shutdown) = shutdown {
        let _ = shutdown.send(());
    }
    if let Some(task) = task {
        let _ = task.await;
    }
}

pub(super) fn seed_frames(
    mut buffered: Vec<HostFrame>,
    current_policy: &Policy,
    credentials: Vec<Credential>,
) -> Vec<HostFrame> {
    if buffered.iter().any(|f| matches!(f, HostFrame::Policy(_))) {
        return buffered;
    }
    buffered.insert(0, initial_policy_frame(current_policy, credentials));
    buffered
}

pub(super) fn buffer_frame(pending: &mut Vec<HostFrame>, frame: HostFrame) {
    if matches!(frame, HostFrame::Policy(_)) {
        pending.retain(|f| !matches!(f, HostFrame::Policy(_)));
    }
    pending.push(frame);
}

// constant-time compare — do not replace with ==
pub(super) fn validate_authorization_header(header: &[u8], expected: &[u8]) -> bool {
    if header.len() != expected.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in header.iter().zip(expected.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub(super) fn is_expected_close(e: &tungstenite::Error) -> bool {
    matches!(
        e,
        tungstenite::Error::Protocol(
            tungstenite::error::ProtocolError::ResetWithoutClosingHandshake
        ) | tungstenite::Error::ConnectionClosed
    )
}

fn generate_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval_flow::session::tests::{CapturingStore, RecordingNotifier};
    use crate::audit::{AnchorSink, AuditLog};
    use crate::test_env::EnvVarGuard;
    use lns_ipc::Anchor;
    use lns_policy::RouteRule;
    use std::time::Duration;
    use tempfile::tempdir;

    struct FixedClock(u64);
    impl crate::oauth::Clock for FixedClock {
        fn now_unix(&self) -> u64 {
            self.0
        }
    }
    static CLOCK: FixedClock = FixedClock(1_700_000_000);

    #[derive(Default)]
    struct MemLog {
        bytes: Vec<u8>,
        syncs: usize,
    }

    impl AuditLog for MemLog {
        async fn append_synced(&mut self, bytes: &[u8]) -> Result<()> {
            self.bytes.extend_from_slice(bytes);
            self.syncs += 1;
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemAnchor {
        anchors: Vec<Anchor>,
    }

    impl AnchorSink for MemAnchor {
        fn write(&mut self, anchor: &Anchor) -> Result<()> {
            self.anchors.push(anchor.clone());
            Ok(())
        }
    }

    fn home_for(temp: &tempfile::TempDir) -> EnvVarGuard {
        EnvVarGuard::set("HOME", temp.path())
    }

    fn session_with_dummy_sink() -> Arc<ApprovalSession> {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        Arc::new(ApprovalSession::new(
            Policy::default(),
            Policy::default(),
            notifier,
            store,
            tx,
            Duration::from_secs(30),
        ))
    }

    fn credential_session_with_dummy_sink() -> Arc<CredentialSession> {
        credential_session_seeding(Vec::new())
    }

    fn credential_session_seeding(
        custom: Vec<crate::credential_flow::providers::DefProvider>,
    ) -> Arc<CredentialSession> {
        use crate::credential_flow::notification::NoopCredentialNotifier;
        use crate::credential_flow::store::{CredentialStateFile, JsonFileCredentialStore};
        // Real store (not an inline fake) keeps its trait impl out of the coverage gap.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let store = Arc::new(JsonFileCredentialStore::new(dir.path().join("creds.json")));
        // Leak the tempdir to keep it alive for the session; fine in test-only code.
        Box::leak(Box::new(dir));
        let (tx, _rx) = mpsc::unbounded_channel();
        Arc::new(
            CredentialSession::new(
                CredentialStateFile::new(),
                Arc::new(NoopCredentialNotifier),
                store,
                tx,
                Duration::from_secs(30),
            )
            .with_custom_providers(Arc::new(custom)),
        )
    }

    #[test]
    fn token_is_64_hex_chars() {
        let t = generate_token();
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn token_is_unique_across_calls() {
        assert_ne!(generate_token(), generate_token());
    }

    #[test]
    fn validate_auth_accepts_matching_bearer() {
        assert!(validate_authorization_header(
            b"Bearer abc123",
            b"Bearer abc123"
        ));
    }

    #[test]
    fn validate_auth_rejects_length_mismatch_and_byte_mismatch() {
        assert!(!validate_authorization_header(b"", b"Bearer abc"));
        assert!(!validate_authorization_header(b"Bearer abc", b"Bearer abd"));
        assert!(!validate_authorization_header(
            b"Bearer abc",
            b"Bearer abcd"
        ));
        assert!(!validate_authorization_header(
            b"Bearer abc ",
            b"Bearer abc"
        ));
    }

    #[test]
    fn validate_auth_accepts_empty_pair() {
        assert!(validate_authorization_header(b"", b""));
    }

    fn decision_frame(id: &str, decision: crate::approval_flow::protocol::Decision) -> HostFrame {
        use crate::approval_flow::protocol::RequestDecision;
        HostFrame::RequestDecision(RequestDecision {
            id: id.into(),
            decision,
        })
    }

    fn policy_with_rule(host: &str) -> HostFrame {
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host(host));
        initial_policy_frame(&p, Vec::new())
    }

    #[test]
    fn buffer_frame_appends_decision_frames_in_arrival_order() {
        use crate::approval_flow::protocol::Decision;
        let mut buf: Vec<HostFrame> = Vec::new();
        buffer_frame(&mut buf, decision_frame("r1", Decision::AllowOnce));
        buffer_frame(&mut buf, decision_frame("r2", Decision::DenyOnce));
        buffer_frame(&mut buf, decision_frame("r3", Decision::AllowAlways));

        assert_eq!(
            buf,
            vec![
                decision_frame("r1", Decision::AllowOnce),
                decision_frame("r2", Decision::DenyOnce),
                decision_frame("r3", Decision::AllowAlways),
            ],
            "each RequestDecision must be preserved in arrival order"
        );
    }

    #[test]
    fn buffer_frame_coalesces_consecutive_policy_frames_keeping_latest() {
        let mut buf: Vec<HostFrame> = Vec::new();
        buffer_frame(&mut buf, policy_with_rule("first.example"));
        buffer_frame(&mut buf, policy_with_rule("second.example"));
        buffer_frame(&mut buf, policy_with_rule("third.example"));

        assert_eq!(
            buf,
            vec![policy_with_rule("third.example")],
            "buffer must collapse to one Policy frame carrying the latest snapshot"
        );
    }

    #[test]
    fn buffer_frame_preserves_decision_frames_when_collapsing_policy_frames() {
        use crate::approval_flow::protocol::Decision;
        let mut buf: Vec<HostFrame> = Vec::new();

        buffer_frame(
            &mut buf,
            initial_policy_frame(&Policy::default(), Vec::new()),
        );
        buffer_frame(&mut buf, decision_frame("r1", Decision::AllowOnce));
        buffer_frame(&mut buf, policy_with_rule("later.example"));
        buffer_frame(&mut buf, decision_frame("r2", Decision::DenyOnce));

        assert_eq!(
            buf,
            vec![
                decision_frame("r1", Decision::AllowOnce),
                policy_with_rule("later.example"),
                decision_frame("r2", Decision::DenyOnce),
            ],
            "stale Policy frames coalesce away; decisions and the newer \
             Policy survive in arrival order"
        );
    }

    #[test]
    fn seed_frames_prepends_initial_policy_when_buffer_has_no_policy() {
        use crate::approval_flow::protocol::Decision;
        let mut current = Policy::default();
        current.add_rule(RouteRule::allow_host("current.example"));
        let buffered = vec![
            decision_frame("r1", Decision::AllowOnce),
            decision_frame("r2", Decision::DenyOnce),
        ];
        let seeded = seed_frames(buffered, &current, Vec::new());

        assert_eq!(
            seeded,
            vec![
                initial_policy_frame(&current, Vec::new()),
                decision_frame("r1", Decision::AllowOnce),
                decision_frame("r2", Decision::DenyOnce),
            ],
            "an initial Policy carrying the current session state \
             must lead the seeded set, with decisions following in \
             arrival order"
        );
    }

    #[test]
    fn seed_frames_uses_buffered_policy_without_prepending_a_second_one() {
        use crate::approval_flow::protocol::Decision;
        let buffered = vec![
            decision_frame("r1", Decision::AllowOnce),
            policy_with_rule("hotswap.example"),
            decision_frame("r2", Decision::DenyOnce),
        ];
        let seeded = seed_frames(buffered.clone(), &Policy::default(), Vec::new());

        let policy_count = seeded
            .iter()
            .filter(|f| matches!(f, HostFrame::Policy(_)))
            .count();
        assert_eq!(
            policy_count, 1,
            "exactly one Policy frame must lead each connection; \
             buffered hot-swap must not be duplicated by an initial send"
        );
        assert_eq!(
            seeded, buffered,
            "buffered ordering is preserved when a Policy is already present"
        );
    }

    #[test]
    fn seed_frames_on_empty_buffer_emits_just_the_initial_policy() {
        let mut current = Policy::default();
        current.add_rule(RouteRule::allow_host("fresh.example"));
        let seeded = seed_frames(Vec::new(), &current, Vec::new());
        assert_eq!(seeded.len(), 1);
        assert!(matches!(seeded[0], HostFrame::Policy(_)));
    }

    #[test]
    fn initial_policy_frame_carries_session_network() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("api.linear.app"));
        let frame = initial_policy_frame(&p, Vec::new());
        let json = serde_json::to_value(&frame).expect("serialise");
        assert_eq!(json["type"], "policy");
        assert_eq!(
            json["network"]["egress"]["http"][0]["match"],
            "api.linear.app"
        );
    }

    #[test]
    fn initial_policy_frame_carries_registry_credentials_so_supervisor_seeds_env_at_boot() {
        let p = Policy::default();
        let creds = vec![Credential {
            id: "some-provider".into(),
            env_var: Some("SOME_TOKEN".into()),
            placeholder: Some("some-placeholder-0000".into()),
            injections: Vec::new(),
        }];
        let frame = initial_policy_frame(&p, creds);
        let json = serde_json::to_value(&frame).expect("serialise");
        let credentials = &json["credentials"];
        assert!(
            credentials.is_array(),
            "credentials must be present on the initial Policy frame, got {json}"
        );
        let ids: Vec<&str> = credentials
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"some-provider"), "got {ids:?}");
    }

    #[tokio::test]
    async fn accept_loop_seeded_policy_includes_credentials_from_credential_session() {
        use crate::credential_flow::providers::DefProvider;
        use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef};
        let session = session_with_dummy_sink();
        let credential_session = credential_session_seeding(vec![DefProvider::new(ProviderDef {
            id: "some-provider".into(),
            env_var: "SOME_TOKEN".into(),
            placeholder: "some-placeholder-0000".into(),
            injections: vec![InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: "api.some-provider.example".into(),
                header: None,
            }],
        })]);

        let seeded = seed_frames(
            Vec::new(),
            &session.current_policy(),
            current_credentials(&credential_session),
        );

        assert_eq!(seeded.len(), 1);
        let json = serde_json::to_value(&seeded[0]).expect("serialise");
        assert_eq!(json["type"], "policy");
        let ids: Vec<&str> = json["credentials"]
            .as_array()
            .expect("credentials array present on the seeded Policy")
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"some-provider"), "got {ids:?}");
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn audit_path_resolves_the_jsonl_path_without_creating_the_run_dir() {
        let temp = tempdir().unwrap();
        let _home = home_for(&temp);
        let path = audit_path("aa42").expect("audit_path");
        assert!(path.ends_with("runs/aa42/audit.jsonl"));
        assert!(
            !path.parent().unwrap().exists(),
            "the run dir is created lazily on the first audit event, not at spawn"
        );
    }

    #[tokio::test]
    async fn handle_inbound_audit_event_appends_with_prev_hash_chain() {
        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        let stop = handle_inbound(
            Message::Text(
                r#"{"type":"audit_event","action":"GET http://api.example.test/","result":"success"}"#
                    .into(),
            ),
            &session,
            &credential_session_with_dummy_sink(),
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &AuditBudget::with_defaults(),
            &CLOCK,
        )
        .await
        .expect("handle_inbound");
        assert!(!stop, "audit_event keeps the read loop running");
        assert_eq!(log.syncs, 1, "each appended audit line is fsync'd");
        let written = String::from_utf8(log.bytes).unwrap();
        assert!(written.ends_with('\n'), "unterminated record: {written:?}");
        assert!(written.contains("prev_hash"), "unchained record: {written}");
        let obj: Value = serde_json::from_str(written.trim_end()).unwrap();
        assert_eq!(
            obj["unmapped"]["lns_ts"],
            crate::time_fmt::rfc3339_from_unix(1_700_000_000),
            "the host clock stamps the time on every recorded audit event; got: {written}"
        );
        assert_eq!(
            anchor.anchors.len(),
            1,
            "each appended audit line updates the anchor"
        );
        assert_eq!(anchor.anchors[0].line_count, 1);
        assert_eq!(anchor.anchors[0].head_hash, chain.head().unwrap());
    }

    #[tokio::test]
    async fn a_guest_cannot_forge_host_attribution_through_a_smuggled_unmapped_block() {
        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        let stop = handle_inbound(
            Message::Text(
                r#"{"type":"audit_event","action":"GET http://api.example.test/","result":"success","source":"host","origin":"host","unmapped":{"lns_kind":"approval","lns_origin":"host","lns_decision":"allow_always","lns_ts":"2000-01-01T00:00:00Z"}}"#
                    .into(),
            ),
            &session,
            &credential_session_with_dummy_sink(),
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &AuditBudget::with_defaults(),
            &CLOCK,
        )
        .await
        .expect("handle_inbound");
        assert!(!stop);
        let written = String::from_utf8(log.bytes).unwrap();
        let obj: Value = serde_json::from_str(written.trim_end()).unwrap();
        assert_eq!(
            obj["unmapped"]["lns_kind"], "egress",
            "a guest audit_event is always recorded as guest-proxy egress, never a host kind: {written}"
        );
        assert_eq!(obj["unmapped"]["lns_origin"], "guest-proxy");
        assert_eq!(
            obj["unmapped"]["lns_ts"],
            crate::time_fmt::rfc3339_from_unix(1_700_000_000),
            "the host clock stamps the time; the guest cannot backdate the row: {written}"
        );
        assert_eq!(obj["unmapped"]["lns_run"], "test-run");
        assert!(
            obj["unmapped"].get("lns_decision").is_none(),
            "smuggled approval fields are dropped, never carried into the chain: {written}"
        );
        assert!(
            obj.get("source").is_none(),
            "guest-supplied top-level provenance is discarded: {written}"
        );
        assert_eq!(anchor.anchors.len(), 1);
    }

    #[tokio::test]
    async fn handle_inbound_drops_a_guest_audit_event_that_is_not_an_egress_record() {
        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        let stop = handle_inbound(
            Message::Text(
                r#"{"type":"audit_event","event":"run_env","env":{"GITHUB_TOKEN":"forged"}}"#
                    .into(),
            ),
            &session,
            &credential_session_with_dummy_sink(),
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &AuditBudget::with_defaults(),
            &CLOCK,
        )
        .await
        .expect("handle_inbound");
        assert!(!stop, "a dropped frame must not kill the relay");
        assert!(
            log.bytes.is_empty(),
            "only egress records (which carry an `action`) are accepted from the guest; a frame trying to forge a host-authored env row never reaches the chain"
        );
        assert!(
            anchor.anchors.is_empty(),
            "a dropped frame must not advance the anchor"
        );
    }

    #[tokio::test]
    async fn write_run_env_event_records_injected_env_as_a_chain_entry() {
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        write_run_env_event(
            &["CLAUDE_CODE_USE_BEDROCK=1".into()],
            &[],
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &CLOCK,
        )
        .await
        .expect("write_run_env_event");
        let written = String::from_utf8(log.bytes).unwrap();
        let obj: Value = serde_json::from_str(written.trim_end()).unwrap();
        assert_eq!(obj["class_uid"], 1007, "OCSF Process Activity: {written}");
        assert_eq!(obj["unmapped"]["lns_kind"], "env");
        assert_eq!(obj["device"]["name"], "calm-finch");
        assert_eq!(
            obj["unmapped"]["lns_env"]["CLAUDE_CODE_USE_BEDROCK"],
            crate::workload_env::REDACTED_ENV_VALUE,
            "the recorded env carries names, not raw -e values: {written}"
        );
        assert_eq!(
            obj["unmapped"]["lns_origin"], "host",
            "the host-authored run_env event must carry the host origin: {written}"
        );
        assert!(written.contains("prev_hash"), "must chain: {written}");
        assert!(written.ends_with('\n'));
        assert_eq!(anchor.anchors.len(), 1, "run_env line updates the anchor");
    }

    #[tokio::test]
    async fn handle_inbound_translates_a_guest_egress_frame_into_ocsf() {
        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        handle_inbound(
            Message::Text(
                r#"{"type":"audit_event","action":"GET http://api.example.test:443/","status_code":200,"result":"success","metadata":{"reason":"user-allowed-once"}}"#
                    .into(),
            ),
            &session,
            &credential_session_with_dummy_sink(),
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &AuditBudget::with_defaults(),
            &CLOCK,
        )
        .await
        .expect("handle_inbound");
        let written = String::from_utf8(log.bytes).unwrap();
        let obj: Value = serde_json::from_str(written.trim_end()).unwrap();
        assert_eq!(obj["class_uid"], 4002, "OCSF HTTP Activity: {written}");
        assert_eq!(obj["http_request"]["http_method"], "GET");
        assert_eq!(
            obj["http_request"]["url"]["text"],
            "http://api.example.test:443/"
        );
        assert_eq!(obj["http_response"]["code"], 200);
        assert_eq!(obj["unmapped"]["lns_result"], "success");
        assert_eq!(obj["unmapped"]["lns_origin"], "guest-proxy");
        assert_eq!(anchor.anchors.len(), 1);
    }

    #[tokio::test]
    async fn a_guest_egress_frame_carries_the_client_endpoint_and_process_through_to_ocsf() {
        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        handle_inbound(
            Message::Text(
                r#"{"type":"audit_event","action":"GET http://api.example.test:443/","result":"success","src_endpoint":{"ip":"10.42.0.31","port":37494},"actor":{"process":{"name":"curl","pid":57}}}"#
                    .into(),
            ),
            &session,
            &credential_session_with_dummy_sink(),
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &AuditBudget::with_defaults(),
            &CLOCK,
        )
        .await
        .expect("handle_inbound");
        let obj: Value =
            serde_json::from_str(String::from_utf8(log.bytes).unwrap().trim_end()).unwrap();
        assert_eq!(obj["src_endpoint"]["ip"], "10.42.0.31");
        assert_eq!(obj["src_endpoint"]["port"], 37494);
        assert_eq!(obj["actor"]["process"]["name"], "curl");
        assert_eq!(obj["actor"]["process"]["pid"], 57);
    }

    #[tokio::test]
    async fn a_tls_bridge_frame_yields_a_clean_destination_not_a_garbled_host() {
        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        handle_inbound(
            Message::Text(
                r#"{"type":"audit_event","action":"TLS_BRIDGE example.com -> 1.2.3.4:443","host":"example.com","result":"success","metadata":{"host":"example.com","tls_bridge":true}}"#
                    .into(),
            ),
            &session,
            &credential_session_with_dummy_sink(),
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &AuditBudget::with_defaults(),
            &CLOCK,
        )
        .await
        .expect("handle_inbound");
        let obj: Value =
            serde_json::from_str(String::from_utf8(log.bytes).unwrap().trim_end()).unwrap();
        assert_eq!(
            obj["dst_endpoint"]["domain"], "example.com",
            "the TLS_BRIDGE host, not the `host -> addr` action tail, is the destination domain: {obj}"
        );
        assert!(
            obj["dst_endpoint"].get("port").is_none(),
            "the mangled `example.com -> 1.2.3.4:443` no longer leaks a bogus port: {obj}"
        );
        assert_eq!(obj["http_request"]["url"]["text"], "example.com");
        assert_eq!(obj["unmapped"]["lns_kind"], "egress");
    }

    #[tokio::test]
    async fn write_run_env_event_writes_nothing_when_no_user_env() {
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        write_run_env_event(
            &[],
            &[],
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &CLOCK,
        )
        .await
        .expect("write_run_env_event");
        assert!(log.bytes.is_empty(), "no injected env → no audit line");
        assert!(
            anchor.anchors.is_empty(),
            "no audit line → no anchor update"
        );
    }

    #[tokio::test]
    async fn handle_inbound_credential_pending_routes_into_credential_session() {
        use crate::approval_flow::protocol::CredentialPending;
        use crate::credential_flow::session::{CredentialNotifier, CredentialPendingPrompt};
        use crate::credential_flow::store::{CredentialStateFile, JsonFileCredentialStore};
        use std::sync::Mutex as StdMutex;

        #[derive(Default)]
        struct Recorder {
            seen: StdMutex<Vec<String>>,
        }
        impl CredentialNotifier for Recorder {
            fn present(&self, p: &CredentialPendingPrompt) {
                self.seen.lock().unwrap().push(p.credential_id.clone());
            }
            fn dismiss(&self, _: &str) {}
            fn inform(&self, _: &str) {}
            fn clear_informs(&self) {}
        }
        // Real store (not an inline fake) keeps its trait impl out of the coverage gap.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let store = Arc::new(JsonFileCredentialStore::new(dir.path().join("creds.json")));
        let notifier = Arc::new(Recorder::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let credential_session = Arc::new(CredentialSession::new(
            CredentialStateFile::new(),
            notifier.clone(),
            store,
            tx,
            Duration::from_secs(30),
        ));

        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();

        let raw = r#"{"type":"credential_pending","id":"c1","credentialId":"some-provider","action":"use of some-provider placeholder","reason":"placeholder-unauthorized"}"#;
        let exhausted_budget = AuditBudget::new(0, 0);
        let stop = handle_inbound(
            Message::Text(raw.into()),
            &session,
            &credential_session,
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &exhausted_budget,
            &CLOCK,
        )
        .await
        .unwrap();

        assert!(!stop);
        let seen = notifier.seen.lock().unwrap();
        assert_eq!(seen.as_slice(), &["some-provider".to_string()]);
        assert!(
            log.bytes.is_empty(),
            "credential_pending must not hit audit chain"
        );
        assert!(anchor.anchors.is_empty());
        let _ = CredentialPending {
            id: "c1".into(),
            credential_id: "some-provider".into(),
            action: "x".into(),
            reason: "y".into(),
        };
        // Exercise the rest of the notifier trait so its empty bodies aren't a coverage hole.
        notifier.dismiss("unused");
        notifier.inform("unused");
        notifier.clear_informs();
    }

    #[tokio::test]
    async fn handle_inbound_request_pending_routes_into_session() {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let session = Arc::new(ApprovalSession::new(
            Policy::default(),
            Policy::default(),
            notifier.clone(),
            store,
            tx,
            Duration::from_secs(30),
        ));
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();

        let raw = r#"{"type":"request_pending","id":"r1","host":"api.linear.app","action":"CONNECT api.linear.app:443","reason":"policy-ambiguous"}"#;
        let stop = handle_inbound(
            Message::Text(raw.into()),
            &session,
            &credential_session_with_dummy_sink(),
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &AuditBudget::with_defaults(),
            &CLOCK,
        )
        .await
        .unwrap();

        assert!(!stop);
        let presented = notifier.presented.lock().unwrap();
        assert_eq!(presented.len(), 1);
        assert_eq!(presented[0].id, "r1");
        assert_eq!(presented[0].host, "api.linear.app");
    }

    #[tokio::test]
    async fn handle_inbound_close_terminates_loop_without_writing() {
        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        let stop = handle_inbound(
            Message::Close(None),
            &session,
            &credential_session_with_dummy_sink(),
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &AuditBudget::with_defaults(),
            &CLOCK,
        )
        .await
        .expect("handle_inbound");
        assert!(stop, "Close must end the read loop");
        assert!(
            log.bytes.is_empty(),
            "Close must not append to the audit log"
        );
        assert!(anchor.anchors.is_empty());
    }

    #[tokio::test]
    async fn handle_inbound_non_audit_text_without_request_pending_is_dropped() {
        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        let stop = handle_inbound(
            Message::Text(r#"{"type":"something_else"}"#.into()),
            &session,
            &credential_session_with_dummy_sink(),
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &AuditBudget::with_defaults(),
            &CLOCK,
        )
        .await
        .expect("handle_inbound");
        assert!(!stop);
        assert!(log.bytes.is_empty());
        assert!(anchor.anchors.is_empty());
    }

    #[tokio::test]
    async fn handle_inbound_binary_and_ping_are_no_op() {
        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        for msg in [
            Message::Binary(b"raw".to_vec().into()),
            Message::Ping(b"keep-alive".to_vec().into()),
            Message::Pong(b"keep-alive".to_vec().into()),
        ] {
            let stop = handle_inbound(
                msg,
                &session,
                &credential_session_with_dummy_sink(),
                &mut AuditWriter {
                    chain: &mut chain,
                    log: &mut log,
                    anchor: &mut anchor,
                    run: "test-run",
                    microvm: "calm-finch",
                },
                &AuditBudget::with_defaults(),
                &CLOCK,
            )
            .await
            .expect("handle_inbound");
            assert!(!stop);
        }
        assert!(log.bytes.is_empty());
        assert!(anchor.anchors.is_empty());
    }

    #[tokio::test]
    async fn handle_inbound_drops_malformed_audit_event_without_killing_relay() {
        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        let stop = handle_inbound(
            Message::Text(r#"this is not json but "type":"audit_event" appears"#.into()),
            &session,
            &credential_session_with_dummy_sink(),
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &AuditBudget::with_defaults(),
            &CLOCK,
        )
        .await
        .expect("malformed audit_event must NOT propagate as an error");
        assert!(!stop, "loop continues after a malformed line");
        assert!(
            log.bytes.is_empty(),
            "malformed audit_event must not be appended to the chain"
        );
        assert!(anchor.anchors.is_empty());
    }

    #[tokio::test]
    async fn handle_inbound_request_pending_with_audit_event_substring_in_action_is_not_misrouted()
    {
        let notifier = Arc::new(crate::approval_flow::session::tests::RecordingNotifier::default());
        let store = Arc::new(crate::approval_flow::session::tests::CapturingStore::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let session = Arc::new(ApprovalSession::new(
            Policy::default(),
            Policy::default(),
            notifier.clone(),
            store,
            tx,
            Duration::from_secs(30),
        ));
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();

        let raw = r#"{"type":"request_pending","id":"r1","host":"api.linear.app","action":"POST /v1/log {\"type\":\"audit_event\"}","reason":"policy-ambiguous"}"#;
        let stop = handle_inbound(
            Message::Text(raw.into()),
            &session,
            &credential_session_with_dummy_sink(),
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &AuditBudget::with_defaults(),
            &CLOCK,
        )
        .await
        .unwrap();

        assert!(!stop);
        assert!(
            log.bytes.is_empty(),
            "request_pending must NOT be appended to the audit chain just because \
             its body embeds an `audit_event` substring; got: {:?}",
            log.bytes
        );
        assert!(anchor.anchors.is_empty());
        let presented = notifier.presented.lock().unwrap();
        assert_eq!(presented.len(), 1, "the prompt must reach the developer");
        assert_eq!(presented[0].id, "r1");
    }

    #[tokio::test]
    async fn handle_inbound_unknown_type_is_dropped_without_attempting_audit_chain() {
        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        let stop = handle_inbound(
            Message::Text(r#"{"type":"watch","payload":{}}"#.into()),
            &session,
            &credential_session_with_dummy_sink(),
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &AuditBudget::with_defaults(),
            &CLOCK,
        )
        .await
        .expect("unknown type must not error");
        assert!(!stop);
        assert!(
            log.bytes.is_empty(),
            "unknown type must not hit the audit chain"
        );
        assert!(anchor.anchors.is_empty());
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn spawn_returns_relay_with_ws_url_token_and_audit_path() {
        let temp = tempdir().unwrap();
        let _home = home_for(&temp);
        let session = session_with_dummy_sink();
        let (_tx, frame_rx) = mpsc::unbounded_channel::<HostFrame>();
        let relay = spawn(
            "aa1234",
            "calm-finch",
            session,
            credential_session_with_dummy_sink(),
            frame_rx,
            vec![],
        )
        .expect("spawn");
        assert_eq!(
            relay.url,
            format!("vsock://host:{VSOCK_PORT}/v1/sandbox"),
            "supervisor's LENS_SANDBOX_WS_URL has a fixed loopback shape"
        );
        assert_eq!(relay.token.len(), 64, "64-hex-char bearer token");
        assert!(
            relay
                .audit_path
                .to_string_lossy()
                .ends_with("runs/aa1234/audit.jsonl")
        );
        drop(relay.fd_tx);
        tokio::task::yield_now().await;
    }

    #[test]
    fn is_expected_close_true_for_reset_without_closing_handshake() {
        let e = tungstenite::Error::Protocol(
            tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
        );
        assert!(is_expected_close(&e));
    }

    #[test]
    fn is_expected_close_true_for_connection_closed() {
        let e = tungstenite::Error::ConnectionClosed;
        assert!(is_expected_close(&e));
    }

    #[test]
    fn is_expected_close_false_for_io_error() {
        let e = tungstenite::Error::Io(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "broken pipe",
        ));
        assert!(!is_expected_close(&e));
    }

    #[test]
    fn is_expected_close_false_for_other_protocol_errors() {
        let e =
            tungstenite::Error::Protocol(tungstenite::error::ProtocolError::HandshakeIncomplete);
        assert!(!is_expected_close(&e));
    }

    #[test]
    fn audit_budget_rejects_once_the_event_ceiling_is_reached() {
        let budget = AuditBudget::new(2, 1_000_000);
        assert!(budget.try_charge(10));
        assert!(budget.try_charge(10));
        assert!(
            !budget.try_charge(10),
            "a run cannot append past its per-run event ceiling"
        );
    }

    #[test]
    fn audit_budget_rejects_once_the_byte_ceiling_would_be_exceeded() {
        let budget = AuditBudget::new(100, 50);
        assert!(budget.try_charge(40));
        assert!(
            !budget.try_charge(20),
            "a charge crossing the byte ceiling is refused and consumes nothing"
        );
        assert!(
            budget.try_charge(10),
            "a smaller charge that still fits is accepted after the refusal"
        );
    }

    #[test]
    fn audit_budget_is_shared_across_clones_so_the_ceiling_is_per_run_not_per_connection() {
        let budget = AuditBudget::new(2, 1_000_000);
        let reconnect = budget.clone();
        assert!(budget.try_charge(10));
        assert!(
            reconnect.try_charge(10),
            "a reconnecting connection draws from the same per-run budget"
        );
        assert!(
            !reconnect.try_charge(10),
            "reconnecting cannot mint a fresh budget — the per-run ceiling holds across connections"
        );
    }

    #[tokio::test]
    async fn handle_inbound_audit_event_over_budget_is_dropped_without_fsync_or_anchor() {
        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        let budget = AuditBudget::new(0, 1_000_000);
        let stop = handle_inbound(
            Message::Text(
                r#"{"type":"audit_event","action":"GET http://api.example.test/","result":"success"}"#
                    .into(),
            ),
            &session,
            &credential_session_with_dummy_sink(),
            &mut AuditWriter {
                chain: &mut chain,
                log: &mut log,
                anchor: &mut anchor,
                run: "test-run",
                microvm: "calm-finch",
            },
            &budget,
            &CLOCK,
        )
        .await
        .expect("handle_inbound");
        assert!(
            !stop,
            "dropping an over-budget event must not kill the relay"
        );
        assert_eq!(log.syncs, 0, "an over-budget audit_event is never appended");
        assert!(log.bytes.is_empty());
        assert!(
            anchor.anchors.is_empty(),
            "an over-budget drop must not advance the anchor"
        );
    }

    #[tokio::test]
    async fn supersede_connection_waits_for_the_prior_task_to_finish_before_returning() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let finished = Arc::new(AtomicBool::new(false));
        let finished_c = finished.clone();
        let handle = tokio::spawn(async move {
            let _ = shutdown_rx.await;
            finished_c.store(true, Ordering::SeqCst);
        });
        supersede_connection(Some(shutdown_tx), Some(handle)).await;
        assert!(
            finished.load(Ordering::SeqCst),
            "the prior connection must fully relinquish the chain — finishing any in-flight \
             append — before the superseding connection proceeds"
        );
    }

    #[tokio::test]
    async fn supersede_connection_without_a_prior_connection_is_a_noop() {
        supersede_connection(None, None).await;
    }
}
