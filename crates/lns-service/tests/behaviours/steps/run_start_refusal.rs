use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cucumber::{given, then, when};
use lns_ipc::{Response, RunImageArgs};
use lns_service::ipc::RunHost;
use tokio::io::AsyncWriteExt;

use crate::world::BehaviourWorld;

/// A run host scripted with what `prepare` answers, recording whether the run was ever served.
struct ScriptedHost {
    refusal: Option<String>,
    never_finishes: bool,
    served: Arc<AtomicBool>,
}

impl RunHost for ScriptedHost {
    type Prepared = ();

    async fn prepare(&self, _run_id: &str, _args: &RunImageArgs) -> anyhow::Result<()> {
        // Pending once before answering, so the start has a poll in which to notice its client — real preparation reaches the network, and one that answers without ever yielding would hide the watch entirely.
        tokio::task::yield_now().await;
        if self.never_finishes {
            std::future::pending::<()>().await;
        }
        match &self.refusal {
            Some(message) => Err(anyhow::anyhow!("{message}")),
            None => Ok(()),
        }
    }

    async fn serve<W>(
        &self,
        _stream: &mut W,
        _run_id: String,
        _args: RunImageArgs,
        _prepared: (),
    ) -> anyhow::Result<()>
    where
        W: AsyncWriteExt + Unpin + Send,
    {
        self.served.store(true, Ordering::SeqCst);
        Ok(())
    }
}

const COLLISION: &str =
    "the sandbox and mixins/debug-tools both publish host port 18080; one of them has to move it";

fn args_named(name: &str) -> RunImageArgs {
    RunImageArgs {
        image: Some("registry.example.test/some-sandbox:1".into()),
        resolved_image: None,
        mixins: Vec::new(),
        composed_mixins: Vec::new(),
        name: Some(name.to_string()),
        cpus: 1,
        mem: 0,
        cpus_explicit: false,
        mem_explicit: false,
        cpus_config: None,
        mem_config: None,
        sandbox_user: None,
        sandbox_uid: None,
        entrypoint: None,
        hostname: None,
        cmd: Vec::new(),
        env: Vec::new(),
        workdir: None,
        debug: false,
        tty: false,
        stdin: false,
        initial_winsize: None,
        detached: false,
        published_ports: Vec::new(),
        volumes: Vec::new(),
        binds: Vec::new(),
        auto_remove: false,
        verify_sandbox: false,
        definition: None,
        definition_dir: None,
        authored_egress: None,
        packed_filesets: Vec::new(),
        denied_host_paths: Vec::new(),
    }
}

#[given("a run whose sources publish one host port twice")]
fn a_colliding_run(w: &mut BehaviourWorld) {
    w.start_refusal = Some(COLLISION.to_string());
}

#[given("a run nothing refuses")]
fn a_run_nothing_refuses(w: &mut BehaviourWorld) {
    w.start_refusal = None;
}

#[when(regex = r#"^the run request asks for the name "([^"]+)"$"#)]
async fn the_run_request_asks_for_a_name(w: &mut BehaviourWorld, name: String) {
    drive_start(w, &name).await;
}

#[then("the service answers that the name is in use")]
fn the_service_answers_the_name_is_in_use(w: &mut BehaviourWorld) -> Result<(), String> {
    match w.start_frames.first() {
        Some(Response::Error { message }) if message.contains("already in use") => Ok(()),
        other => Err(format!(
            "a name another run holds has to be answered before this one starts; got {other:?}"
        )),
    }
}

#[then("the run is not served")]
fn the_run_is_not_served(w: &mut BehaviourWorld) -> Result<(), String> {
    if w.start_served {
        return Err("the refused run was served, so it took a registry entry and a name".into());
    }
    Ok(())
}

#[when("the run request arrives")]
async fn the_run_request_arrives(w: &mut BehaviourWorld) {
    let name = format!("start-refusal-{}", w.start_refusal.is_some());
    drive_start(w, &name).await;
}

async fn drive_start(w: &mut BehaviourWorld, name: &str) {
    let served = Arc::new(AtomicBool::new(false));
    let host = ScriptedHost {
        refusal: w.start_refusal.clone(),
        never_finishes: false,
        served: served.clone(),
    };
    let (mut client, mut server) = tokio::io::duplex(64 * 1024);
    lns_service::ipc::start_run(&mut server, args_named(name), &host)
        .await
        .expect("a refused start is an answer, not a transport failure");
    drop(server);

    w.start_served = served.load(Ordering::SeqCst);
    w.start_name = Some(name.to_string());
    let mut frames = Vec::new();
    while let Ok(bytes) = lns_ipc::read_frame_bytes_async(&mut client).await {
        if let Ok(response) = lns_ipc::decode_frame::<Response, _>(&mut bytes.as_slice()) {
            frames.push(response);
        }
    }
    w.start_frames = frames;
}

#[then("the service answers with the refusal, naming the port")]
fn the_service_answers_with_an_error(w: &mut BehaviourWorld) -> Result<(), String> {
    match w.start_frames.first() {
        Some(Response::Error { message }) if message.contains("18080") => Ok(()),
        other => Err(format!(
            "a refusal the client cannot read as a refusal reads as a run that started and died; got {other:?}"
        )),
    }
}

#[then("the client is never told a run started")]
fn no_run_started_frame(w: &mut BehaviourWorld) -> Result<(), String> {
    if w.start_frames
        .iter()
        .any(|f| matches!(f, Response::RunStarted { .. }))
    {
        return Err(format!(
            "a run id the client can attach to is a run that started; got {:?}",
            w.start_frames
        ));
    }
    Ok(())
}

#[then("no run is registered")]
fn no_run_is_registered(w: &mut BehaviourWorld) -> Result<(), String> {
    if w.start_served {
        return Err("the refused run was served, so it took a registry entry and a name".into());
    }
    let name = w.start_name.as_deref().expect("the request named the run");
    lns_service::run_registry::ensure_name_available(name)
        .map_err(|held| format!("the refused run still holds its name: {held}"))
}

#[then("the run is served")]
fn the_run_is_served(w: &mut BehaviourWorld) -> Result<(), String> {
    if w.start_served {
        Ok(())
    } else {
        Err("a run nothing refuses has to reach the step that registers it".into())
    }
}

#[given("a run whose preparation never finishes")]
fn a_run_that_never_prepares(w: &mut BehaviourWorld) {
    w.start_never_finishes = true;
}

#[when("the client goes away before the run starts")]
async fn the_client_goes_away(w: &mut BehaviourWorld) {
    let served = Arc::new(AtomicBool::new(false));
    let host = ScriptedHost {
        refusal: None,
        never_finishes: w.start_never_finishes,
        served: served.clone(),
    };
    let (client, mut server) = tokio::io::duplex(64 * 1024);
    drop(client);
    let returned = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        lns_service::ipc::start_run(&mut server, args_named("gone-before-start"), &host),
    )
    .await;
    w.start_returned = returned.is_ok();
    w.start_served = served.load(Ordering::SeqCst);
}

#[then("the run start gives up and serves nothing")]
fn the_run_start_gives_up(w: &mut BehaviourWorld) -> Result<(), String> {
    if !w.start_returned {
        return Err(
            "a run nobody waits for has to stop being prepared, or its work and its bound ports outlive every way to reach it".into(),
        );
    }
    if w.start_served {
        return Err("a run whose client left was served anyway".into());
    }
    Ok(())
}

#[when("the client sends a stray byte before the run starts")]
async fn the_client_sends_a_stray_byte(w: &mut BehaviourWorld) {
    let served = Arc::new(AtomicBool::new(false));
    let host = ScriptedHost {
        refusal: None,
        never_finishes: false,
        served: served.clone(),
    };
    let (mut client, mut server) = tokio::io::duplex(64 * 1024);
    client.write_all(b"x").await.expect("stray byte");
    let returned = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        lns_service::ipc::start_run(&mut server, args_named("stray-byte"), &host),
    )
    .await;
    w.start_returned = returned.is_ok();
    w.start_served = served.load(Ordering::SeqCst);
}
