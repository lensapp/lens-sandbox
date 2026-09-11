#![cfg(feature = "host-approval")]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lns_openshell_spike::Gate;
use lns_policy::{Policy, PolicyStore};
use lns_service::approval_flow::protocol::{
    Decision, GuestFrame, HostFrame, RequestPending, Treatment,
};
use lns_service::approval_flow::session::{
    ApprovalSession, DecisionOutcome, Notifier, PendingPrompt,
};
use openshell_supervisor_network::opa::NetworkInput;
use tokio::sync::mpsc;

#[derive(Default)]
struct Notifications(Mutex<Vec<String>>);

impl Notifier for Notifications {
    fn present(&self, prompt: &PendingPrompt) {
        self.0
            .lock()
            .unwrap()
            .push(format!("present:{}", prompt.id));
    }
    fn dismiss(&self, id: &str) {
        self.0.lock().unwrap().push(format!("dismiss:{id}"));
    }
    fn expire(&self, id: &str) {
        self.0.lock().unwrap().push(format!("expire:{id}"));
    }
    fn inform(&self, message: &str) {
        self.0.lock().unwrap().push(message.into());
    }
    fn clear_informs(&self) {
        self.0.lock().unwrap().push("clear".into());
    }
}

#[derive(Default)]
struct Store(Mutex<Vec<Policy>>);

impl PolicyStore for Store {
    fn save(&self, policy: &Policy) -> std::io::Result<()> {
        self.0.lock().unwrap().push(policy.clone());
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn real_lns_session_answers_the_held_adapter_request() {
    for (decision, allowed) in [
        (Decision::AllowOnce, true),
        (Decision::DenyOnce, false),
        (Decision::AllowAlways, true),
        (Decision::DenyAlways, false),
    ] {
        let (tx, mut pending_rx) = mpsc::unbounded_channel();
        let gate = Gate::new(tx).unwrap();
        let (frames, mut frame_rx) = mpsc::unbounded_channel();
        let notifications = Arc::new(Notifications::default());
        let store = Arc::new(Store::default());
        let session = ApprovalSession::new(
            Policy::default(),
            Policy::default(),
            notifications.clone(),
            store.clone(),
            frames,
            Duration::from_secs(60),
        );
        let input = NetworkInput {
            host: "api.example.com".into(),
            port: 443,
            binary_path: "/usr/bin/curl".into(),
            binary_sha256: String::new(),
            ancestors: vec![],
            cmdline_paths: vec![],
        };
        let mut authorization = Box::pin(gate.authorize(&input));
        let pending = tokio::select! {
            result = &mut authorization => panic!("completed before the host answered: {result:?}"),
            pending = pending_rx.recv() => pending.unwrap(),
        };
        let bytes = serde_json::to_vec(&GuestFrame::RequestPending(RequestPending {
            id: "spike-request".into(),
            host: pending.host.clone(),
            action: format!("CONNECT {}:{}", pending.host, pending.port),
            reason: "no rule decides".into(),
            treatment: Treatment::Raw,
        }))
        .unwrap();
        let GuestFrame::RequestPending(request) = serde_json::from_slice(&bytes).unwrap() else {
            panic!("request frame")
        };
        session.submit_pending(request, Instant::now());
        assert_eq!(
            session.record_decision("spike-request", decision),
            DecisionOutcome::Resolved
        );
        let always = matches!(decision, Decision::AllowAlways | Decision::DenyAlways);
        let mut policy_seen = false;
        let mut answer = None;
        while let Ok(frame) = frame_rx.try_recv() {
            let bytes = serde_json::to_vec(&frame).unwrap();
            match serde_json::from_slice::<HostFrame>(&bytes).unwrap() {
                HostFrame::Policy(message) => {
                    let policy = serde_json::from_value(
                        serde_json::json!({"network":{"egress":message.network.unwrap().egress}}),
                    )
                    .unwrap();
                    gate.reload(&policy).unwrap();
                    policy_seen = true;
                }
                HostFrame::RequestDecision(reply) => {
                    assert_eq!(reply.id, "spike-request");
                    assert_eq!(reply.decision, decision);
                    assert!(
                        !always || policy_seen,
                        "permanent decision arrived before its policy"
                    );
                    answer = Some(matches!(
                        reply.decision,
                        Decision::AllowOnce | Decision::AllowAlways
                    ));
                }
            }
        }
        pending
            .answer
            .send(answer.expect("host decision frame"))
            .unwrap();
        assert_eq!(authorization.await.unwrap(), allowed);
        if always {
            let next = tokio::time::timeout(Duration::from_secs(1), gate.authorize(&input))
                .await
                .expect("standing policy must decide without another prompt")
                .unwrap();
            assert_eq!(next, allowed);
            assert!(pending_rx.try_recv().is_err());
        }
        assert!(
            notifications
                .0
                .lock()
                .unwrap()
                .contains(&"present:spike-request".into())
        );
        assert_eq!(
            store.0.lock().unwrap().len(),
            usize::from(matches!(
                decision,
                Decision::AllowAlways | Decision::DenyAlways
            ))
        );
    }
}
