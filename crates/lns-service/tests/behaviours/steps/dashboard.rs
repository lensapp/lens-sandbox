use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_ipc::{
    ApprovalAnswer, ApprovalEntryKind, ApprovalInfo, DashboardApproval, DashboardEvent, Response,
};
use lns_service::ipc::dashboard::{Snapshot, snapshot_frames};

#[derive(Debug, Default)]
pub struct DashboardRig {
    snapshot: Snapshot,
    frames: Vec<Response>,
}

fn event(detail: String) -> DashboardEvent {
    DashboardEvent {
        id: "event-1".into(),
        ts: "2026-09-09T12:00:00Z".into(),
        when: "2026-09-09 12:00:00".into(),
        run: "run-1".into(),
        kind: "egress".into(),
        detail,
        raw: "{\"message\":\"CONNECT example.com:443\",\"sequence\":9007199254740993}".into(),
    }
}

#[given("a dashboard with an event and an integrity warning")]
fn audited(world: &mut BehaviourWorld) {
    world.dashboard = Some(DashboardRig {
        snapshot: Snapshot {
            events: vec![event("CONNECT example.com:443".into())],
            warnings: vec!["audit integrity: log truncated or rolled back".into()],
            ..Default::default()
        },
        ..Default::default()
    });
}

#[given("a dashboard whose events exceed one IPC frame in total")]
fn large(world: &mut BehaviourWorld) {
    world.dashboard = Some(DashboardRig {
        snapshot: Snapshot {
            events: (0..20).map(|_| event("x".repeat(100_000))).collect(),
            ..Default::default()
        },
        ..Default::default()
    });
}

#[given("a dashboard with a raw unanswered destination")]
fn asked(world: &mut BehaviourWorld) {
    world.dashboard = Some(DashboardRig {
        snapshot: Snapshot {
            approvals: vec![DashboardApproval {
                entry: ApprovalInfo {
                    id: "entry-1".into(),
                    sandbox: Some("quiet_river".into()),
                    subject: "example.com".into(),
                    action: Some("CONNECT example.com:443".into()),
                    kind: ApprovalEntryKind::Destination,
                    answer: "undecided".into(),
                    answerable: true,
                },
                raw: true,
                answers: vec![ApprovalAnswer::AlwaysAllow, ApprovalAnswer::AlwaysDeny],
                grantable: false,
            }],
            ..Default::default()
        },
        ..Default::default()
    });
}

#[when("a client reads the dashboard frames")]
fn read(world: &mut BehaviourWorld) {
    let rig = world.dashboard.as_mut().unwrap();
    rig.frames = snapshot_frames(std::mem::take(&mut rig.snapshot)).collect();
}

#[then("the dashboard contains the event and its original raw data")]
fn contains_event(world: &mut BehaviourWorld) {
    let frames = &world.dashboard.as_ref().unwrap().frames;
    assert_eq!(
        frames.first(),
        Some(&Response::DashboardBegin),
        "a dashboard read must begin a complete replacement"
    );
    assert!(frames.contains(&Response::DashboardEvent {
        event: event("CONNECT example.com:443".into())
    }));
}

#[then("the integrity warning precedes the dashboard completion")]
fn warned(world: &mut BehaviourWorld) {
    let frames = &world.dashboard.as_ref().unwrap().frames;
    assert_eq!(frames.last(), Some(&Response::DashboardEnd));
    assert!(
        matches!(&frames[frames.len()-2], Response::DashboardWarning { message } if message.contains("truncated"))
    );
}

#[then("every event arrives in a valid bounded IPC frame")]
fn bounded(world: &mut BehaviourWorld) {
    let frames = &world.dashboard.as_ref().unwrap().frames;
    assert_eq!(
        frames
            .iter()
            .filter(|f| matches!(f, Response::DashboardEvent { .. }))
            .count(),
        20,
        "every event must reach the client"
    );
    for frame in frames {
        let bytes = lns_ipc::encode_frame(frame).unwrap();
        assert_eq!(
            lns_ipc::decode_frame::<Response, _>(&mut &bytes[..]).unwrap(),
            *frame
        );
    }
}

#[then("the destination retains its warning and persistent answer choices")]
fn choices(world: &mut BehaviourWorld) {
    assert!(world.dashboard.as_ref().unwrap().frames.iter().any(|frame| matches!(frame,
        Response::DashboardApproval { approval } if approval.raw && approval.answers == [ApprovalAnswer::AlwaysAllow, ApprovalAnswer::AlwaysDeny] && !approval.grantable
    )), "the service must supply the available answers and raw-traffic warning");
}
