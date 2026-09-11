use std::sync::Arc;

use cucumber::{given, then, when};
use lns_ipc::{LiveApprovalAction, LiveApprovalSnapshot, Response};
use lns_service::approval_flow::{
    inbox::{ApprovalInbox, DecisionDelivery, RequestAction},
    protocol::{Decision, Treatment},
    session::PendingPrompt,
};
use tokio::sync::{mpsc, watch};

use crate::world::BehaviourWorld;

pub struct LiveApprovalRig {
    inbox: Arc<ApprovalInbox>,
    sender: mpsc::UnboundedSender<DecisionDelivery>,
    receiver: mpsc::UnboundedReceiver<DecisionDelivery>,
    subscription: Option<watch::Receiver<LiveApprovalSnapshot>>,
    token: String,
}

impl std::fmt::Debug for LiveApprovalRig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveApprovalRig").finish_non_exhaustive()
    }
}

fn prompt(host: &str) -> PendingPrompt {
    PendingPrompt {
        id: "guest-request".into(),
        host: host.into(),
        action: format!("CONNECT {host}:443"),
        treatment: Treatment::Inspected,
        run: Some("sandbox".into()),
        offer: None,
    }
}

#[given(regex = r#"^the service has a live approval for "([^"]+)"$"#)]
fn pending(world: &mut BehaviourWorld, host: String) {
    let inbox = ApprovalInbox::new();
    let (sender, receiver) = mpsc::unbounded_channel();
    inbox.insert_pending(prompt(&host), sender.clone());
    world.live_approval = Some(LiveApprovalRig {
        inbox,
        sender,
        receiver,
        subscription: None,
        token: String::new(),
    });
}

#[when("a client subscribes to live approvals")]
fn subscribe(world: &mut BehaviourWorld) {
    let rig = world.live_approval.as_mut().unwrap();
    let subscription = rig.inbox.watch();
    let snapshot = subscription.borrow().clone();
    assert_eq!(
        snapshot.approvals.len(),
        1,
        "a late subscriber must receive the pending approval"
    );
    rig.token = snapshot.approvals[0].token.clone();
    rig.subscription = Some(subscription);
}

#[then(regex = r#"^the client sees the live approval for "([^"]+)"$"#)]
fn observes(world: &mut BehaviourWorld, host: String) {
    let rig = world.live_approval.as_ref().unwrap();
    let snapshot = rig.subscription.as_ref().unwrap().borrow();
    assert_eq!(snapshot.approvals[0].host, host);
    assert!(snapshot.approvals[0].waiting);
}

#[when("the client allows the live request once")]
fn allow(world: &mut BehaviourWorld) {
    let rig = world.live_approval.as_ref().unwrap();
    assert_eq!(
        rig.inbox.respond(&rig.token, LiveApprovalAction::AllowOnce),
        Response::LiveApprovalSubmitted
    );
}

#[then("the decision reaches the waiting run exactly once")]
fn delivered(world: &mut BehaviourWorld) {
    let rig = world.live_approval.as_mut().unwrap();
    let delivery = rig.receiver.try_recv().unwrap();
    assert_eq!(delivery.id, "guest-request");
    assert_eq!(delivery.action, RequestAction::Decide(Decision::AllowOnce));
    assert!(rig.receiver.try_recv().is_err());
    let snapshot = rig.subscription.as_ref().unwrap().borrow();
    assert!(
        snapshot.approvals.is_empty(),
        "other subscribers must observe the answer"
    );
}

#[then("a second answer to the same presentation is stale")]
fn stale(world: &mut BehaviourWorld) {
    let rig = world.live_approval.as_ref().unwrap();
    assert_eq!(
        rig.inbox.respond(&rig.token, LiveApprovalAction::DenyOnce),
        Response::LiveApprovalStale
    );
}

#[then("a newly connected client sees no live approvals")]
fn empty(world: &mut BehaviourWorld) {
    assert!(
        world
            .live_approval
            .as_ref()
            .unwrap()
            .inbox
            .watch()
            .borrow()
            .approvals
            .is_empty()
    );
}

#[when("the request ends and another request reuses its identifier")]
fn replace(world: &mut BehaviourWorld) {
    let rig = world.live_approval.as_ref().unwrap();
    rig.inbox.remove_pending("guest-request");
    rig.inbox
        .insert_pending(prompt("replacement.example.com"), rig.sender.clone());
}

#[then("a newly connected client still sees the replacement request")]
fn replacement(world: &mut BehaviourWorld) {
    let rig = world.live_approval.as_ref().unwrap();
    assert_eq!(
        rig.inbox.watch().borrow().approvals[0].host,
        "replacement.example.com"
    );
}

#[when("the run stops receiving decisions")]
fn disconnect(world: &mut BehaviourWorld) {
    world.live_approval.as_mut().unwrap().receiver.close();
}

#[then("submitting the live answer reports the disconnected run")]
fn disconnected(world: &mut BehaviourWorld) {
    let rig = world.live_approval.as_ref().unwrap();
    assert!(
        matches!(rig.inbox.respond(&rig.token, LiveApprovalAction::AllowOnce), Response::Error { message } if message.contains("no longer"))
    );
}
