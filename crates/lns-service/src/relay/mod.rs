use anyhow::{Context, Result};
use serde_json::Value;
use std::os::fd::RawFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::Instrument;

use crate::approval_flow::protocol::{Credential, GuestFrame, HostFrame, PolicyMessage};
use crate::approval_flow::session::ApprovalSession;
use crate::credential_flow::registry::expand_credentials_for_wire_with_custom;
use crate::credential_flow::session::CredentialSession;
use lns_policy::Policy;

mod adapter;

pub const VSOCK_PORT: u32 = 1024;

fn audit_path(run_id: u32) -> Result<PathBuf> {
    let path = lns_ipc::audit_log_for_run(&format!("{run_id}"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    Ok(path)
}

pub struct Relay {
    pub url: String,
    pub token: String,
    pub audit_path: PathBuf,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fd_tx: mpsc::UnboundedSender<RawFd>,
}

pub fn spawn(
    run_id: u32,
    session: Arc<ApprovalSession>,
    credential_session: Arc<CredentialSession>,
    frame_rx: mpsc::UnboundedReceiver<HostFrame>,
    user_env: Vec<String>,
) -> Result<Relay> {
    let token = generate_token();
    let url = format!("vsock://host:{VSOCK_PORT}/v1/sandbox");
    let audit = audit_path(run_id).context("creating audit log dir")?;
    let (fd_tx, fd_rx) = mpsc::unbounded_channel::<RawFd>();

    let token_clone = token.clone();
    let audit_clone = audit.clone();

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
        network: Some(policy.network.clone()),
        credentials: Some(credentials),
    })
}

pub(super) fn current_credentials(session: &CredentialSession) -> Vec<Credential> {
    expand_credentials_for_wire_with_custom(&session.current_state(), session.custom_providers())
}

async fn write_chain_line<L: crate::audit::AuditLog, S: crate::audit::AnchorSink>(
    bytes: &[u8],
    chain: &lns_ipc::AuditChain,
    audit_file: &mut L,
    anchor_sink: &mut S,
) -> Result<()> {
    let mut line = Vec::with_capacity(bytes.len() + 1);
    line.extend_from_slice(bytes);
    line.push(b'\n');
    audit_file.append_synced(&line).await?;
    if let Some(anchor) = chain.anchor() {
        anchor_sink.write(&anchor)?;
    }
    Ok(())
}

pub(super) async fn write_run_env_event<L: crate::audit::AuditLog, S: crate::audit::AnchorSink>(
    user_env: &[String],
    chain: &mut lns_ipc::AuditChain,
    audit_file: &mut L,
    anchor_sink: &mut S,
) -> Result<()> {
    let Some(obj) = crate::workload_env::injected_env_audit(user_env) else {
        return Ok(());
    };
    let bytes = chain.augment_obj(obj);
    write_chain_line(&bytes, chain, audit_file, anchor_sink).await
}

const HOST_RESERVED_EVENTS: &[&str] = &["run_env"];

fn stamp_guest_audit_event(
    mut obj: serde_json::Map<String, Value>,
) -> Option<serde_json::Map<String, Value>> {
    let event = obj.get("event").and_then(Value::as_str);
    if let Some(event) = event
        && HOST_RESERVED_EVENTS.contains(&event)
    {
        crate::log::warn!(
            reserved_event = event,
            "rejected guest audit_event with a host-reserved event value"
        );
        return None;
    }
    obj.remove("source");
    obj.remove("origin");
    obj.insert(
        "origin".to_string(),
        Value::String("guest-proxy".to_string()),
    );
    Some(obj)
}

pub(super) async fn handle_inbound<L: crate::audit::AuditLog, S: crate::audit::AnchorSink>(
    msg: Message,
    session: &Arc<ApprovalSession>,
    credential_session: &Arc<CredentialSession>,
    chain: &mut lns_ipc::AuditChain,
    audit_file: &mut L,
    anchor_sink: &mut S,
) -> Result<bool> {
    let text = match msg {
        Message::Text(text) => text,
        Message::Close(_) => return Ok(true),
        _ => return Ok(false),
    };
    // Dispatch on the parsed top-level `type` so a `request_pending` embedding the literal `"type":"audit_event"` can't be misrouted into the audit chain.
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(text.as_str()) else {
        return Ok(false);
    };
    let ty = obj.get("type").and_then(Value::as_str);
    match ty {
        Some("audit_event") => {
            let Some(stamped) = stamp_guest_audit_event(obj) else {
                return Ok(false);
            };
            let bytes = chain.augment_obj(stamped);
            write_chain_line(&bytes, chain, audit_file, anchor_sink).await?;
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
            notifier,
            store,
            tx,
            Duration::from_secs(30),
        ))
    }

    fn credential_session_with_dummy_sink() -> Arc<CredentialSession> {
        use crate::credential_flow::notification::NoopCredentialNotifier;
        use crate::credential_flow::store::{CredentialStateFile, JsonFileCredentialStore};
        // Real store (not an inline fake) keeps its trait impl out of the coverage gap.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let store = Arc::new(JsonFileCredentialStore::new(dir.path().join("creds.json")));
        // Leak the tempdir to keep it alive for the session; fine in test-only code.
        Box::leak(Box::new(dir));
        let (tx, _rx) = mpsc::unbounded_channel();
        Arc::new(CredentialSession::new(
            CredentialStateFile::new(),
            Arc::new(NoopCredentialNotifier),
            store,
            tx,
            Duration::from_secs(30),
        ))
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
            json["network"]["allowedRoutes"][0]["match"],
            "api.linear.app"
        );
    }

    #[test]
    fn initial_policy_frame_carries_registry_credentials_so_supervisor_seeds_env_at_boot() {
        use crate::credential_flow::providers;
        let p = Policy::default();
        let creds: Vec<_> = providers::ALL
            .iter()
            .map(|pd| Credential {
                id: pd.id().into(),
                env_var: Some(pd.env_var().into()),
                placeholder: Some(pd.placeholder().into()),
                injections: Vec::new(),
            })
            .collect();
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
        for expected in ["github", "openai", "anthropic", "linear"] {
            assert!(ids.contains(&expected), "missing {expected} in {ids:?}");
        }
    }

    #[tokio::test]
    async fn accept_loop_seeded_policy_includes_credentials_from_credential_session() {
        let session = session_with_dummy_sink();
        let credential_session = credential_session_with_dummy_sink();

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
        for expected in ["github", "openai", "anthropic", "linear"] {
            assert!(ids.contains(&expected), "missing {expected} in {ids:?}");
        }
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn audit_path_creates_run_directory_and_returns_jsonl_path() {
        let temp = tempdir().unwrap();
        let _home = home_for(&temp);
        let path = audit_path(42).expect("audit_path");
        assert!(path.ends_with("runs/42/audit.jsonl"));
        let parent = path.parent().unwrap();
        assert!(parent.is_dir());
    }

    #[tokio::test]
    async fn handle_inbound_audit_event_appends_with_prev_hash_chain() {
        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        let stop = handle_inbound(
            Message::Text(r#"{"type":"audit_event","route":"deny"}"#.into()),
            &session,
            &credential_session_with_dummy_sink(),
            &mut chain,
            &mut log,
            &mut anchor,
        )
        .await
        .expect("handle_inbound");
        assert!(!stop, "audit_event keeps the read loop running");
        assert_eq!(log.syncs, 1, "each appended audit line is fsync'd");
        let written = String::from_utf8(log.bytes).unwrap();
        assert!(
            written.ends_with('\n'),
            "every appended record must terminate with a newline so \
             the JSONL tail consumer can split lines: {written:?}"
        );
        assert!(
            written.contains("prev_hash"),
            "the chain must augment each line with a prev_hash field; got: {written}"
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
    async fn handle_inbound_audit_event_stamps_guest_proxy_origin_and_strips_guest_provenance() {
        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        let stop = handle_inbound(
            Message::Text(
                r#"{"type":"audit_event","route":"deny","source":"host","origin":"host"}"#.into(),
            ),
            &session,
            &credential_session_with_dummy_sink(),
            &mut chain,
            &mut log,
            &mut anchor,
        )
        .await
        .expect("handle_inbound");
        assert!(!stop);
        let written = String::from_utf8(log.bytes).unwrap();
        let obj: Value = serde_json::from_str(written.trim_end()).unwrap();
        assert_eq!(
            obj.get("origin").and_then(Value::as_str),
            Some("guest-proxy"),
            "host must stamp every guest audit_event with the guest-proxy origin: {written}"
        );
        assert!(
            obj.get("source").is_none(),
            "guest-supplied `source` provenance must be stripped: {written}"
        );
        assert_eq!(
            anchor.anchors.len(),
            1,
            "a stamped guest event is still written to the chain"
        );
    }

    #[tokio::test]
    async fn handle_inbound_audit_event_with_reserved_run_env_event_is_rejected_and_warns() {
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
            &mut chain,
            &mut log,
            &mut anchor,
        )
        .await
        .expect("handle_inbound");
        assert!(!stop, "a rejected forgery must not kill the relay");
        assert!(
            log.bytes.is_empty(),
            "a guest event claiming the reserved `run_env` type must not reach the chain"
        );
        assert!(
            anchor.anchors.is_empty(),
            "a rejected forgery must not advance the anchor"
        );
    }

    #[tokio::test]
    async fn handle_inbound_audit_event_with_non_reserved_event_value_is_stamped_and_written() {
        let session = session_with_dummy_sink();
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        handle_inbound(
            Message::Text(r#"{"type":"audit_event","event":"net_connect","host":"x"}"#.into()),
            &session,
            &credential_session_with_dummy_sink(),
            &mut chain,
            &mut log,
            &mut anchor,
        )
        .await
        .expect("handle_inbound");
        let written = String::from_utf8(log.bytes).unwrap();
        let obj: Value = serde_json::from_str(written.trim_end()).unwrap();
        assert_eq!(
            obj.get("event").and_then(Value::as_str),
            Some("net_connect")
        );
        assert_eq!(
            obj.get("origin").and_then(Value::as_str),
            Some("guest-proxy"),
            "a guest event whose `event` value is not host-reserved is still stamped and written: {written}"
        );
        assert_eq!(anchor.anchors.len(), 1);
    }

    #[tokio::test]
    async fn write_run_env_event_records_injected_env_as_a_chain_entry() {
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        write_run_env_event(
            &["CLAUDE_CODE_USE_BEDROCK=1".into()],
            &mut chain,
            &mut log,
            &mut anchor,
        )
        .await
        .expect("write_run_env_event");
        let written = String::from_utf8(log.bytes).unwrap();
        assert!(written.contains("run_env"), "got: {written}");
        assert!(
            written.contains("CLAUDE_CODE_USE_BEDROCK"),
            "got: {written}"
        );
        let obj: Value = serde_json::from_str(written.trim_end()).unwrap();
        assert_eq!(
            obj.get("origin").and_then(Value::as_str),
            Some("host"),
            "the host-authored run_env event must carry the host origin: {written}"
        );
        assert!(written.contains("prev_hash"), "must chain: {written}");
        assert!(written.ends_with('\n'));
        assert_eq!(anchor.anchors.len(), 1, "run_env line updates the anchor");
    }

    #[tokio::test]
    async fn write_run_env_event_writes_nothing_when_no_user_env() {
        let mut chain = lns_ipc::AuditChain::new();
        let mut log = MemLog::default();
        let mut anchor = MemAnchor::default();
        write_run_env_event(&[], &mut chain, &mut log, &mut anchor)
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

        let raw = r#"{"type":"credential_pending","id":"c1","credentialId":"github","action":"use of github placeholder","reason":"placeholder-unauthorized"}"#;
        let stop = handle_inbound(
            Message::Text(raw.into()),
            &session,
            &credential_session,
            &mut chain,
            &mut log,
            &mut anchor,
        )
        .await
        .unwrap();

        assert!(!stop);
        let seen = notifier.seen.lock().unwrap();
        assert_eq!(seen.as_slice(), &["github".to_string()]);
        assert!(
            log.bytes.is_empty(),
            "credential_pending must not hit audit chain"
        );
        assert!(anchor.anchors.is_empty());
        let _ = CredentialPending {
            id: "c1".into(),
            credential_id: "github".into(),
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
            &mut chain,
            &mut log,
            &mut anchor,
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
            &mut chain,
            &mut log,
            &mut anchor,
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
            &mut chain,
            &mut log,
            &mut anchor,
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
                &mut chain,
                &mut log,
                &mut anchor,
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
            &mut chain,
            &mut log,
            &mut anchor,
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
            &mut chain,
            &mut log,
            &mut anchor,
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
            &mut chain,
            &mut log,
            &mut anchor,
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
            1234,
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
                .ends_with("runs/1234/audit.jsonl")
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
}
