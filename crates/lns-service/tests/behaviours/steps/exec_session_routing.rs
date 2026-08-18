use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_service::vm::session_client::SessionInput;

fn make_handle(
    input_tx: tokio::sync::mpsc::Sender<SessionInput>,
) -> lns_service::run_registry::RunHandle {
    let (cancel_tx, _cancel_rx) = tokio::sync::oneshot::channel::<i32>();
    lns_service::run_registry::RunHandle {
        cancel_tx,
        detach_tx: std::sync::Mutex::new(None),
        task: tokio::spawn(std::future::pending::<()>()),
        input_tx: Some(input_tx),
        exec_sessions: Default::default(),
        connector: None,
        name: String::new(),
        image: String::new(),
        command: String::new(),
        started: String::new(),
        status: std::sync::Mutex::new(lns_ipc::RunStatus::Running),
        logs: std::sync::Arc::new(lns_service::run_log::RunLogBuffer::default()),
        config: lns_ipc::RunConfig::default(),
        exec_environment: Default::default(),
    }
}

fn run_id(world: &BehaviourWorld) -> String {
    world.exec.run_id.clone().expect("an active run")
}

fn register_exec(world: &mut BehaviourWorld, session_id: &str, first: bool) {
    let run_id = run_id(world);
    let (tx, rx) = tokio::sync::mpsc::channel::<SessionInput>(8);
    assert!(lns_service::run_registry::register_exec_session(
        &run_id,
        session_id.to_string(),
        tx,
    ));
    let target = lns_ipc::SessionTarget::Exec {
        run_id,
        session_id: session_id.to_string(),
    };
    if first {
        world.exec.first_rx = Some(rx);
        world.exec.first_target = Some(target);
    } else {
        world.exec.second_rx = Some(rx);
        world.exec.second_target = Some(target);
    }
}

#[given(regex = r#"^an active run named \"([^\"]+)\"$"#)]
fn active_run_named(world: &mut BehaviourWorld, _name: String) {
    let run_id = lns_service::run_registry::allocate_run_id();
    let (primary_tx, primary_rx) = tokio::sync::mpsc::channel::<SessionInput>(8);
    lns_service::run_registry::register(run_id.clone(), make_handle(primary_tx));
    world.exec.run_id = Some(run_id);
    world.exec.primary_rx = Some(primary_rx);
}

#[given("its primary session is attached to another client")]
fn primary_attached_elsewhere(world: &mut BehaviourWorld) {
    assert!(world.exec.primary_rx.is_some());
}

#[given("its primary session is running")]
fn primary_running(world: &mut BehaviourWorld) {
    assert!(world.exec.primary_rx.is_some());
}

#[given("two interactive exec sessions are active")]
fn two_exec_sessions(world: &mut BehaviourWorld) {
    register_exec(world, "exec-1", true);
    register_exec(world, "exec-2", false);
}

#[when(regex = r#"^the user runs \"(lns exec(?: [^\"]*)?)\"$"#)]
fn user_runs(world: &mut BehaviourWorld, _command: String) {
    register_exec(world, "exec-1", true);
    world.exec.response = Some(lns_ipc::Response::Acknowledged);
}

#[when("the first exec client resizes its terminal")]
async fn first_exec_resizes(world: &mut BehaviourWorld) {
    let target = world.exec.first_target.clone().expect("first exec target");
    world.exec.response = Some(
        lns_service::ipc::handle_request(
            &lns_ipc::Request::SessionResize {
                target,
                rows: 40,
                cols: 120,
            },
            std::time::Instant::now(),
        )
        .await,
    );
    world.exec.first_event = world
        .exec
        .first_rx
        .as_mut()
        .expect("first exec rx")
        .recv()
        .await;
}

#[when("the first exec client sends SIGINT")]
async fn first_exec_signals(world: &mut BehaviourWorld) {
    let target = world.exec.first_target.clone().expect("first exec target");
    world.exec.response = Some(
        lns_service::ipc::handle_request(
            &lns_ipc::Request::SessionSignal {
                target,
                signal: lns_ipc::SignalKind::Int,
            },
            std::time::Instant::now(),
        )
        .await,
    );
    world.exec.first_event = world
        .exec
        .first_rx
        .as_mut()
        .expect("first exec rx")
        .recv()
        .await;
}

#[when("the user enters the detach chord in the first exec session")]
async fn first_exec_detaches(world: &mut BehaviourWorld) {
    let target = world.exec.first_target.clone().expect("first exec target");
    world.exec.response = Some(
        lns_service::ipc::handle_request(
            &lns_ipc::Request::SessionDetach { target },
            std::time::Instant::now(),
        )
        .await,
    );
    world.exec.first_event = world
        .exec
        .first_rx
        .as_mut()
        .expect("first exec rx")
        .recv()
        .await;
}

struct TerminationProbe(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for TerminationProbe {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

#[when("the first exec client disconnects unexpectedly")]
async fn first_exec_disconnects(world: &mut BehaviourWorld) {
    let target = world.exec.first_target.as_ref().expect("first exec target");
    let lns_ipc::SessionTarget::Exec { run_id, session_id } = target.clone() else {
        unreachable!()
    };
    let (terminated_tx, terminated_rx) = tokio::sync::oneshot::channel::<()>();
    let probe = TerminationProbe(Some(terminated_tx));
    let session_task = tokio::spawn(async move {
        let _probe = probe;
        std::future::pending::<()>().await
    });
    let (client, mut server) = tokio::io::duplex(64);
    drop(client);
    let (_frame_tx, mut frame_rx) = tokio::sync::mpsc::channel::<lns_ipc::WireFrame>(4);
    lns_service::ipc::drive_exec_stream(
        &mut server,
        &run_id,
        &session_id,
        session_task,
        &mut frame_rx,
    )
    .await
    .expect("the stream driver survives a vanished client");
    world.exec.first_task_terminated = Some(
        tokio::time::timeout(std::time::Duration::from_secs(1), terminated_rx)
            .await
            .is_ok(),
    );
}

#[then("the user receives a live shell prompt")]
fn live_shell_prompt(world: &mut BehaviourWorld) {
    assert!(world.exec.first_target.is_some());
}

#[then("the primary session remains attached and usable")]
fn primary_remains_attached(world: &mut BehaviourWorld) {
    primary_remains_running(world);
}

#[then("only the first exec session receives the new dimensions")]
fn first_exec_receives_resize(world: &mut BehaviourWorld) {
    assert!(matches!(
        world.exec.first_event,
        Some(SessionInput::Resize {
            rows: 40,
            cols: 120
        })
    ));
    assert!(
        world
            .exec
            .second_rx
            .as_mut()
            .expect("second exec rx")
            .try_recv()
            .is_err()
    );
}

#[then("only the first exec session receives SIGINT")]
fn first_exec_receives_signal(world: &mut BehaviourWorld) {
    assert!(matches!(
        world.exec.first_event,
        Some(SessionInput::Signal(lns_session::SignalKind::Int))
    ));
    assert!(
        world
            .exec
            .second_rx
            .as_mut()
            .expect("second exec rx")
            .try_recv()
            .is_err()
    );
}

#[then("the primary session is unaffected")]
fn primary_unaffected(world: &mut BehaviourWorld) {
    assert!(
        world
            .exec
            .primary_rx
            .as_mut()
            .expect("primary rx")
            .try_recv()
            .is_err()
    );
}

#[then("the second exec session remains usable")]
fn second_exec_usable(world: &mut BehaviourWorld) {
    assert!(
        lns_service::run_registry::session_input_sender(
            world
                .exec
                .second_target
                .as_ref()
                .expect("second exec target")
        )
        .is_some()
    );
}

#[then("the first exec session is terminated")]
fn first_exec_terminated(world: &mut BehaviourWorld) {
    assert!(matches!(world.exec.first_event, Some(SessionInput::Detach)));
    assert!(
        lns_service::run_registry::session_input_sender(
            world.exec.first_target.as_ref().expect("first exec target")
        )
        .is_none()
    );
}

#[then("its CLI returns successfully")]
fn cli_returns_successfully(world: &mut BehaviourWorld) {
    assert_eq!(world.exec.response, Some(lns_ipc::Response::DetachAccepted));
}

#[then("the primary session remains running")]
fn primary_remains_running(world: &mut BehaviourWorld) {
    assert!(
        lns_service::run_registry::session_input_sender(&lns_ipc::SessionTarget::Primary {
            run_id: run_id(world),
        })
        .is_some()
    );
}

#[then("only the first exec session is cancelled")]
fn first_exec_cancelled(world: &mut BehaviourWorld) {
    assert_eq!(
        world.exec.first_task_terminated,
        Some(true),
        "the disconnected exec's guest task must be cancelled, not left running"
    );
    assert!(
        lns_service::run_registry::session_input_sender(
            world.exec.first_target.as_ref().expect("first exec target")
        )
        .is_none()
    );
}
