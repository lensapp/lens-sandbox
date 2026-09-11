use std::collections::HashMap;
use std::io::IsTerminal;
use std::os::fd::AsFd;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use lns_openshell_spike::launch::real::{self as launch, Launch};
use lns_openshell_spike::{Error, Gate, Pending};
use openshell_core::policy::{
    FilesystemPolicy, NetworkMode, NetworkPolicy, ProcessPolicy, ProxyPolicy, SandboxPolicy,
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

fn report(error: impl std::fmt::Display) -> Error {
    error.to_string().into()
}

pub async fn run() -> Result<i32, Error> {
    // SAFETY: prctl consumes scalar arguments and changes only this supervisor's dumpability.
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| "crypto provider already installed")?;
    let uid: u32 = std::env::var("LENS_RUN_UID")?.parse()?;
    let gid: u32 = std::env::var("LENS_RUN_GID")?.parse()?;
    let namespace = crate::isolation::namespace()?;
    let listener = crate::isolation::listener()?;
    let scripts = crate::script_runner::Scripts::load(uid, gid)?;
    let policy = workload_policy(uid, gid);
    let (pending_tx, pending_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Gate::new(pending_tx)?);
    gate.engine
        .enter_fail_closed("waiting for host policy")
        .map_err(report)?;
    let pid = Arc::new(AtomicU32::new(0));
    let networking = crate::proxy::start(listener, &gate, pid.clone()).await?;
    let (dns_tx, dns_rx) = mpsc::channel(256);
    let mut dns = crate::dns_guest::start(&namespace, gate.clone(), dns_tx)?;
    crate::lifecycle::harden(|| {
        openshell_supervisor_process::sandbox::apply_supervisor_startup_hardening()
            .map_err(|error| error.to_string())
    })?;
    let mut reaper = crate::reaper::real::start()?;
    let (initial_tx, initial_rx) = oneshot::channel();
    let relay_gate = gate.clone();
    let mut relay =
        tokio::spawn(async move { relay_loop(relay_gate, pending_rx, initial_tx, dns_rx).await });
    let env = tokio::time::timeout(Duration::from_secs(30), initial_rx).await??;
    tokio::select! {
        result = scripts.run(&policy, pid.clone(), &env, &namespace) => result?,
        result = &mut relay => return Err(format!("host relay exited during scripts: {result:?}").into()),
        result = &mut reaper => return Err(format!("orphan reaper exited during scripts: {result:?}").into()),
        result = &mut dns => return Err(format!("DNS runtime exited during scripts: {result:?}").into()),
    }
    let workspace = std::env::var("WORKSPACE_PATH")
        .ok()
        .or_else(|| std::env::var("LENS_RUN_HOME").ok())
        .unwrap_or_else(|| "/".into());
    let command = std::env::var("AGENT_COMMAND").unwrap_or_else(|_| "sh".into());
    let terminal = crate::terminal::Mode::from_stdin(std::io::stdin().is_terminal())
        == crate::terminal::Mode::Terminal;
    let signals = crate::signals::Signals::new()?;
    let mut env = env;
    env.entry("TERM".into())
        .or_insert_with(|| if terminal { "xterm-256color" } else { "dumb" }.into());
    env.entry("SHELL".into())
        .or_insert_with(|| "/bin/sh".into());
    env.extend(
        openshell_supervisor_process::child_env::proxy_env_vars(lns_session::isolation::PROXY_URL)
            .map(|(k, v)| (k.to_string(), v)),
    );
    {
        let (cert, bundle) = &networking.ca_file_paths;
        env.extend(
            openshell_supervisor_process::child_env::tls_env_vars(cert, bundle)
                .map(|(k, v)| (k.to_string(), v)),
        );
    }
    let (child, io) = launch::spawn(Launch {
        program: "/bin/sh",
        args: &["-c".into(), command],
        cwd: &workspace,
        env: &env,
        uid,
        gid,
        terminal,
        namespace: Some(namespace.as_fd()),
        policy: &policy,
    })?;
    pid.store(child.pid(), Ordering::Release);
    tokio::select! {
        result = crate::workload::run(child, io, signals) => result,
        result = relay => Err(format!("host relay exited: {result:?}").into()),
        result = reaper => Err(format!("orphan reaper exited: {result:?}").into()),
        result = dns => Err(format!("DNS runtime exited: {result:?}").into()),
    }
}

async fn relay_loop(
    gate: Arc<Gate>,
    mut pending: mpsc::UnboundedReceiver<Pending>,
    initial: oneshot::Sender<HashMap<String, String>>,
    mut dns: mpsc::Receiver<String>,
) -> Result<(), Error> {
    let url = std::env::var("LENS_SANDBOX_WS_URL")?;
    let authority = url
        .strip_prefix("vsock://")
        .ok_or("expected vsock relay URL")?
        .split('/')
        .next()
        .ok_or("missing vsock authority")?;
    let (cid, port) = authority.rsplit_once(':').ok_or("missing vsock port")?;
    let cid = if cid == "host" { 2 } else { cid.parse()? };
    let port = port.parse()?;
    let token = std::env::var("LENS_SANDBOX_TOKEN")?;
    let mut initial = Some(initial);
    loop {
        let stream =
            tokio_vsock::VsockStream::connect(tokio_vsock::VsockAddr::new(cid, port)).await?;
        let mut request = url.replacen("vsock://", "ws://", 1).into_client_request()?;
        request
            .headers_mut()
            .insert("Authorization", format!("Bearer {token}").parse()?);
        let (mut socket, _) = tokio_tungstenite::client_async(request, stream).await?;
        let mut answers = HashMap::new();
        loop {
            tokio::select! {
                request = pending.recv() => {
                    let request = request.ok_or("approval channel closed")?;
                    let id = uuid::Uuid::new_v4().to_string();
                    socket.send(Message::Text(json!({"type":"request_pending", "id":id, "host":format!("{}:{}",request.host,request.port), "action":"CONNECT", "reason":"No deciding network rule", "treatment":"raw"}).to_string().into())).await?;
                    answers.insert(id, request.answer);
                }
                Some(host) = dns.recv() => {
                    socket.send(Message::Text(json!({"type":"audit_event", "action":format!("DNS {host}"), "host":host, "result":"denied", "metadata":{"reason":"DNS policy refused or resolution failed"}}).to_string().into())).await?;
                }
                message = socket.next() => {
                    let Some(message) = message else { break; };
                    match message? {
                        Message::Text(text) => {
                            let frame: Value = serde_json::from_str(&text)?;
                            match frame["type"].as_str() {
                                Some("policy") => {
                                    let env = apply_policy(&gate, &frame)?;
                                    if let Some(initial) = initial.take() { let _ = initial.send(env); }
                                }
                                Some("request_decision") => {
                                    if let Some(id) = frame["id"].as_str() && let Some(answer) = answers.remove(id) {
                                        let _ = answer.send(matches!(frame["decision"].as_str(), Some("allow_once" | "allow_always")));
                                    }
                                }
                                _ => {}
                            }
                        }
                        Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            }
        }
        gate.engine
            .enter_fail_closed("host relay disconnected")
            .map_err(report)?;
        drop(answers);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn apply_policy(gate: &Gate, frame: &Value) -> Result<HashMap<String, String>, Error> {
    if frame["credentials"]
        .as_array()
        .is_some_and(|values| !values.is_empty())
        || frame["files"]
            .as_array()
            .is_some_and(|values| !values.is_empty())
    {
        gate.engine
            .enter_fail_closed("credential/file adapter not wired")
            .map_err(report)?;
        return Err("OpenShell E2E spike has not wired credential/file materialization yet".into());
    }
    let policy: lns_policy::Policy =
        serde_json::from_value(json!({"network":{"egress": frame["network"]["egress"]}}))?;
    gate.reload(&policy)?;
    let env: HashMap<String, String> = if frame["env"].is_object() {
        serde_json::from_value(frame["env"].clone())?
    } else {
        HashMap::new()
    };
    Ok(crate::workload_env::resolve(
        env,
        &std::env::vars().collect(),
    ))
}

pub fn workload_policy(uid: u32, gid: u32) -> SandboxPolicy {
    SandboxPolicy {
        version: 1,
        filesystem: FilesystemPolicy {
            read_write: vec!["/".into()],
            ..Default::default()
        },
        network: NetworkPolicy {
            mode: NetworkMode::Proxy,
            proxy: Some(ProxyPolicy {
                http_addr: Some(std::net::SocketAddr::from(([10, 200, 0, 1], 3128))),
            }),
        },
        process: ProcessPolicy {
            run_as_user: Some(uid.to_string()),
            run_as_group: Some(gid.to_string()),
        },
        landlock: Default::default(),
    }
}
