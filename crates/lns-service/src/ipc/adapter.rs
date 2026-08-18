use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use lns_ipc::{Request, Response, WireFrame, decode_frame, encode_frame, read_frame_bytes_async};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

use crate::log;
use crate::shutdown::Shutdown;
use crate::time_fmt::rfc3339_now;

use super::{
    PostPumpAction, PumpOutcome, handle_request, peer_is_authorized, post_pump_action,
    pump_responses, write_error,
};
use super::{build_session_params, validate_exec};

const PEER_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const PEER_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn run_server(
    socket_path: PathBuf,
    shutdown: Arc<Shutdown>,
    started_at: Instant,
) -> anyhow::Result<()> {
    rebuild_stopped_runs().await;
    let listener = bind_or_replace_stale(&socket_path).await?;

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                let (stream, _) = accept_result.context("accept failed")?;
                if !peer_is_trusted(&stream) {
                    continue;
                }
                let shutdown = shutdown.clone();
                let start = started_at;
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, shutdown, start).await {
                        log::warn!(error = %format_args!("{e:#}"), "ipc connection ended with error");
                    }
                });
            }
            _ = shutdown.wait_async() => {
                break;
            }
        }
    }

    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

async fn bind_or_replace_stale(socket_path: &Path) -> anyhow::Result<UnixListener> {
    match bind_secure(socket_path) {
        Ok(listener) => return Ok(listener),
        Err(e) if e.kind() != io::ErrorKind::AddrInUse => {
            return Err(e).with_context(|| format!("failed to bind {}", socket_path.display()));
        }
        Err(_) => {}
    }

    if is_instance_alive(socket_path).await {
        bail!(
            "service already running (socket responds at {})",
            socket_path.display()
        );
    }

    std::fs::remove_file(socket_path)
        .with_context(|| format!("removing stale socket {}", socket_path.display()))?;

    bind_secure(socket_path).with_context(|| format!("failed to rebind {}", socket_path.display()))
}

fn bind_secure(socket_path: &Path) -> io::Result<UnixListener> {
    let listener = UnixListener::bind(socket_path)?;
    set_socket_mode_0600(socket_path)?;
    Ok(listener)
}

#[cfg(unix)]
fn set_socket_mode_0600(socket_path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_socket_mode_0600(_socket_path: &Path) -> io::Result<()> {
    Ok(())
}

fn peer_is_trusted(stream: &UnixStream) -> bool {
    match stream.peer_cred() {
        Ok(cred) => {
            let authorized = peer_is_authorized(cred.uid(), service_euid());
            if !authorized {
                log::warn!(
                    peer_uid = cred.uid(),
                    "rejected IPC connection from another user"
                );
            }
            authorized
        }
        Err(e) => {
            log::warn!(error = %e, "rejected IPC connection: peer credentials unavailable");
            false
        }
    }
}

fn service_euid() -> u32 {
    // SAFETY: geteuid is reentrant and always succeeds per POSIX.
    unsafe { libc::geteuid() }
}

async fn is_instance_alive(socket_path: &Path) -> bool {
    let probe = async {
        let mut stream = UnixStream::connect(socket_path).await?;
        let frame =
            encode_frame(&Request::Ping).map_err(|e| io::Error::other(format!("encode: {e}")))?;
        stream.write_all(&frame).await?;
        let bytes = read_frame_bytes_async(&mut stream)
            .await
            .map_err(|e| io::Error::other(format!("read: {e}")))?;
        let resp: Response =
            decode_frame(&mut &bytes[..]).map_err(|e| io::Error::other(format!("decode: {e}")))?;
        Ok::<bool, io::Error>(matches!(resp, Response::Pong))
    };
    matches!(timeout(PEER_PROBE_TIMEOUT, probe).await, Ok(Ok(true)))
}

async fn handle_connection(
    mut stream: UnixStream,
    shutdown: Arc<Shutdown>,
    started_at: Instant,
) -> anyhow::Result<()> {
    let bytes = timeout(PEER_REQUEST_TIMEOUT, read_frame_bytes_async(&mut stream))
        .await
        .context("client read timed out")?
        .context("reading request frame")?;

    let request: Request = decode_frame(&mut &bytes[..])?;

    match request {
        Request::RunImage(args) => handle_run(stream, *args).await,
        Request::ExecImage(args) => handle_exec(stream, args).await,
        Request::StartRun { run } => handle_start(stream, run).await,
        Request::RunLogs { run, follow } => handle_logs(stream, run, follow).await,
        Request::AttachRun { run } => handle_attach(stream, run).await,
        Request::RunStats { run } => handle_stats(stream, run).await,
        Request::BeginConnectorSignIn { id } => handle_connector_sign_in(stream, id).await,
        Request::BindConnectorCredential { id } => handle_credential_bind(stream, id).await,
        other => handle_one_shot(stream, other, shutdown, started_at).await,
    }
}

async fn handle_start(mut stream: UnixStream, run: String) -> anyhow::Result<()> {
    super::start_stopped_run(&mut stream, &run, &RealStartHost).await
}

struct RealStartHost;

impl super::StartHost for RealStartHost {
    async fn record(&self, run_id: &str) -> anyhow::Result<crate::run_record::RunRecord> {
        let root = crate::cache::root()?;
        let bytes = tokio::fs::read(crate::run_record::record_path(&root, run_id))
            .await
            .map_err(|_| {
                anyhow::anyhow!("run {run_id}'s state is damaged: its run record is missing")
            })?;
        serde_json::from_slice(&bytes).map_err(|_| {
            anyhow::anyhow!("run {run_id}'s state is damaged: its run record does not parse")
        })
    }

    async fn preflight(&self, record: &crate::run_record::RunRecord) -> anyhow::Result<()> {
        let upper = crate::cache::run_dir(&crate::cache::root()?, &record.run_id).join("upper.img");
        match tokio::fs::metadata(&upper).await {
            Ok(m) if m.len() > 0 => {}
            _ => anyhow::bail!(
                "run {}'s state is damaged: its writable layer is missing from {}",
                record.name,
                upper.display()
            ),
        }
        for volume in &record.args.volumes {
            if let Some(holder) = crate::volume_store::global().holder(&volume.name) {
                anyhow::bail!(
                    "volume {:?} is held by run {}; stop or remove it first",
                    volume.name,
                    lns_ipc::short_run_id(&holder)
                );
            }
        }
        for bind in &record.args.binds {
            if tokio::fs::metadata(&bind.host_source).await.is_err() {
                anyhow::bail!(
                    "bind source {} no longer exists on the host",
                    bind.host_source
                );
            }
        }
        for port in &record.args.published_ports {
            if tokio::net::TcpListener::bind((port.host_ip, port.host_port))
                .await
                .is_err()
            {
                anyhow::bail!("host port {} is already in use", port.host_port);
            }
        }
        Ok(())
    }

    async fn serve<W>(
        &self,
        stream: &mut W,
        record: crate::run_record::RunRecord,
    ) -> anyhow::Result<()>
    where
        W: AsyncWriteExt + Unpin + Send,
    {
        let _ = write_error(
            stream,
            format!("run {} cannot be restarted yet", record.name),
        )
        .await;
        Ok(())
    }
}

async fn handle_stats(mut stream: UnixStream, run: String) -> anyhow::Result<()> {
    let run_id = match crate::run_registry::resolve(&run) {
        Ok(id) => id,
        Err(message) => {
            let _ = write_error(&mut stream, message).await;
            return Ok(());
        }
    };
    let response = match crate::run_registry::connector(&run_id) {
        None => Response::Error {
            message: format!("no active run with id {run_id}"),
        },
        Some(connector) => crate::guest_stats::response_from(
            crate::vm::session_client::capture_session_output(
                connector.as_ref(),
                crate::guest_stats::sample_argv(),
            )
            .await,
        ),
    };
    let frame = encode_frame(&response)?;
    stream.write_all(&frame).await?;
    Ok(())
}

async fn handle_logs(mut stream: UnixStream, run: String, follow: bool) -> anyhow::Result<()> {
    let run_id = match crate::run_registry::resolve(&run) {
        Ok(id) => id,
        Err(message) => {
            let _ = write_error(&mut stream, message).await;
            return Ok(());
        }
    };
    let Some(buffer) = crate::run_registry::log_buffer(&run_id) else {
        let _ = write_error(&mut stream, format!("no active run with id {run_id}")).await;
        return Ok(());
    };
    let started = encode_frame(&Response::RunStarted { run_id })?;
    stream.write_all(&started).await?;
    crate::run_log::stream_to(&buffer, &mut stream, follow, 0).await
}

async fn handle_attach(mut stream: UnixStream, run: String) -> anyhow::Result<()> {
    let run_id = match crate::run_registry::resolve(&run) {
        Ok(id) => id,
        Err(message) => {
            let _ = write_error(&mut stream, message).await;
            return Ok(());
        }
    };
    let Some(buffer) = crate::run_registry::log_buffer(&run_id) else {
        let _ = write_error(&mut stream, format!("no active run with id {run_id}")).await;
        return Ok(());
    };
    let started = encode_frame(&Response::RunStarted { run_id })?;
    stream.write_all(&started).await?;
    let tail = buffer.tail_seq();
    crate::run_log::stream_to(&buffer, &mut stream, true, tail).await
}

/// Drives a connector's interactive sign-in host-side, streaming its progress frames to the client and closing with the terminal response.
async fn handle_connector_sign_in(mut stream: UnixStream, id: String) -> anyhow::Result<()> {
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<Response>();
    let sign_in = run_connector_sign_in(&id, progress_tx);
    tokio::pin!(sign_in);
    let terminal = loop {
        tokio::select! {
            biased;
            Some(resp) = progress_rx.recv() => {
                stream.write_all(&encode_frame(&resp)?).await?;
            }
            res = &mut sign_in => break res,
        }
    };
    while let Ok(resp) = progress_rx.try_recv() {
        stream.write_all(&encode_frame(&resp)?).await?;
    }
    stream.write_all(&encode_frame(&terminal)?).await?;
    Ok(())
}

/// Presents a connect-time value-decision card on the approval window and closes the stream with the terminal response.
async fn handle_credential_bind(mut stream: UnixStream, id: String) -> anyhow::Result<()> {
    let terminal = run_credential_bind(&id).await;
    stream.write_all(&encode_frame(&terminal)?).await?;
    Ok(())
}

const CREDENTIAL_BIND_TIMEOUT: Duration = Duration::from_secs(300);

/// Drive a credential connector's value decision through the approval window card and persist the outcome — a value, host-detect, or an explicit deny — to the per-machine store.
async fn run_credential_bind(id: &str) -> Response {
    use crate::credential_flow::bind::{
        BindResolution, bind_prompt, bind_provider, resolve_bind_decision,
    };
    use crate::credential_flow::store::{
        CredentialStore, JsonFileCredentialStore, default_credentials_path,
    };

    let user = lns_policy::connectors::Catalog::load_or_default(
        &lns_policy::connectors::default_connectors_path(),
    )
    .unwrap_or_default();
    let catalog = lns_policy::connectors::effective_connectors(&user);
    let Some(integ) = catalog.iter().find(|i| i.id == id) else {
        return Response::CredentialBindFailed {
            reason: format!("{id:?} is not in this machine's connector catalog"),
        };
    };
    let Some(prompt) = bind_prompt(integ) else {
        return Response::CredentialBindFailed {
            reason: format!("{id:?} is not a credential connector"),
        };
    };
    // Fail fast instead of holding the CLI on a card a headless service can never show.
    if !crate::tray::display_present() {
        return Response::CredentialBindFailed {
            reason: "no display is available for the approval-window value decision; \
                     bind on a machine where the Lens Sandbox window can appear"
                .into(),
        };
    }
    let Some(window_state) = crate::approval_flow::window::get() else {
        return Response::CredentialBindFailed {
            reason: "the approval window is not available to make the value decision".into(),
        };
    };
    let prompt_id = prompt.id.clone();
    let providers: Vec<_> = bind_provider(integ).into_iter().collect();
    let host_value_available =
        crate::credential_flow::registry::detect_for_with(id, &providers).is_some();
    let (decision_tx, mut decision_rx) = mpsc::unbounded_channel();
    // Check-and-insert atomically: a second concurrent bind for the same id would otherwise coalesce into the first card and, on its own timeout, tear the first client's pending card down.
    if !window_state.try_insert_credential_pending(prompt, host_value_available, decision_tx) {
        return Response::CredentialBindFailed {
            reason: format!("a bind for {id:?} is already pending in the approval window"),
        };
    }
    if let Some(ctx) = crate::approval_flow::window::ctx() {
        ctx.request_repaint();
    }
    let delivery = match timeout(CREDENTIAL_BIND_TIMEOUT, decision_rx.recv()).await {
        Ok(Some(delivery)) => delivery,
        Ok(None) | Err(_) => {
            window_state.remove_credential_pending(&prompt_id);
            return Response::CredentialBindFailed {
                reason: "the value decision timed out before it was made".into(),
            };
        }
    };
    match resolve_bind_decision(delivery.request) {
        BindResolution::Persist(entry, decision) => {
            let store = JsonFileCredentialStore::new(default_credentials_path());
            let mut state = match store.load() {
                Ok(state) => state,
                Err(e) => {
                    return Response::CredentialBindFailed {
                        reason: format!("reading the credential store failed: {e}"),
                    };
                }
            };
            state.insert(id.to_string(), entry);
            match store.save(&state) {
                Ok(()) => Response::CredentialBindComplete { decision },
                Err(e) => Response::CredentialBindFailed {
                    reason: format!("storing the decision failed: {e}"),
                },
            }
        }
        BindResolution::Failed(reason) => Response::CredentialBindFailed { reason },
    }
}

/// Drive a connector's oauth sign-in host-side — dispatching on its `flow`, emitting progress via `progress`, persisting the obtained credential — and return the terminal response. Shared by the connect verb and the run launch gate.
pub(crate) async fn run_connector_sign_in(
    id: &str,
    progress: tokio::sync::mpsc::UnboundedSender<Response>,
) -> Response {
    use lns_policy::connectors::OauthFlow;

    let user = lns_policy::connectors::Catalog::load_or_default(
        &lns_policy::connectors::default_connectors_path(),
    )
    .unwrap_or_default();
    let catalog = lns_policy::connectors::effective_connectors(&user);
    let Some(oauth) = catalog
        .iter()
        .find(|i| i.id == id)
        .and_then(|i| i.oauth.as_ref())
    else {
        return Response::OauthSignInFailed {
            reason: format!("{id:?} is not an oauth connector"),
        };
    };
    match oauth.flow {
        OauthFlow::Device => {
            device_sign_in(id, crate::oauth::OauthConfig::from(oauth), progress).await
        }
        OauthFlow::Pkce => pkce_sign_in(id, crate::oauth::PkceConfig::from(oauth), progress).await,
    }
}

/// Emits the device-flow verification prompt and persists the obtained token set.
async fn device_sign_in(
    id: &str,
    cfg: crate::oauth::OauthConfig,
    progress: tokio::sync::mpsc::UnboundedSender<Response>,
) -> Response {
    use crate::oauth::{
        DeviceCode, RealClock, RealDeviceFlow, SignIn, SignInPivot, run_device_flow,
    };

    let outcome = run_device_flow(
        &RealDeviceFlow,
        &cfg,
        move |code: &DeviceCode| {
            let _ = progress.send(Response::OauthVerification {
                verification_uri: code.verification_uri.clone(),
                user_code: code.user_code.clone(),
                expires_in_secs: code.expires_in.as_secs(),
            });
        },
        std::future::pending::<SignInPivot>(),
    )
    .await;

    match outcome {
        Ok(SignIn::Completed(token)) => match persist_oauth_token(id, &token, &RealClock) {
            Ok(()) => Response::OauthSignInComplete,
            Err(e) => Response::OauthSignInFailed {
                reason: format!("storing the token failed: {e}"),
            },
        },
        Ok(SignIn::Denied) => Response::OauthSignInFailed {
            reason: "access was denied".into(),
        },
        Ok(SignIn::Expired) => Response::OauthSignInFailed {
            reason: "the device code expired before authorization".into(),
        },
        Ok(SignIn::Cancelled) => Response::OauthSignInFailed {
            reason: "the sign-in was cancelled".into(),
        },
        // This streamed path offers no token-paste surface, so a pivot can't arise here.
        Ok(SignIn::Token(_)) => Response::OauthSignInFailed {
            reason: "token fallback is not available over this sign-in path".into(),
        },
        Err(e) => Response::OauthSignInFailed {
            reason: format!("{e:#}"),
        },
    }
}

/// Opens the browser to the PKCE authorization URL (emitting it too), then exchanges the redirect's code for the provider's key and persists it as a durable credential.
async fn pkce_sign_in(
    id: &str,
    cfg: crate::oauth::PkceConfig,
    progress: tokio::sync::mpsc::UnboundedSender<Response>,
) -> Response {
    use crate::oauth::{
        PkceChallenge, PkceSignIn, RealAuthCodeFlow, RealCallbackListener, run_pkce_flow,
    };

    let challenge = PkceChallenge::generate();
    let outcome = run_pkce_flow(
        &RealAuthCodeFlow,
        &RealCallbackListener,
        &cfg,
        &challenge,
        move |url: &str| {
            let _ = progress.send(Response::OauthBrowserOpened {
                authorization_url: url.to_string(),
            });
            crate::browser::open(url);
        },
        std::future::pending::<()>(),
        crate::credential_flow::session::PKCE_SIGN_IN_TIMEOUT,
    )
    .await;

    match outcome {
        Ok(PkceSignIn::Completed(key)) => match persist_pkce_key(id, key) {
            Ok(()) => Response::OauthSignInComplete,
            Err(e) => Response::OauthSignInFailed {
                reason: format!("storing the key failed: {e}"),
            },
        },
        Ok(PkceSignIn::Cancelled) => Response::OauthSignInFailed {
            reason: "the sign-in was cancelled".into(),
        },
        Ok(PkceSignIn::TimedOut) => Response::OauthSignInFailed {
            reason: "the sign-in timed out before the browser redirected back".into(),
        },
        Err(e) => Response::OauthSignInFailed {
            reason: format!("{e:#}"),
        },
    }
}

fn persist_oauth_token(
    id: &str,
    token: &crate::oauth::TokenSet,
    clock: &dyn crate::oauth::Clock,
) -> std::io::Result<()> {
    use crate::credential_flow::store::{
        CredentialStore, JsonFileCredentialStore, default_credentials_path,
    };
    let store = JsonFileCredentialStore::new(default_credentials_path());
    let mut state = store.load()?;
    state.insert(id.to_string(), crate::oauth::entry_from_token(clock, token));
    store.save(&state)
}

fn persist_pkce_key(id: &str, key: String) -> std::io::Result<()> {
    use crate::credential_flow::store::{
        CredentialEntry, CredentialStore, JsonFileCredentialStore, default_credentials_path,
    };
    let store = JsonFileCredentialStore::new(default_credentials_path());
    let mut state = store.load()?;
    state.insert(id.to_string(), CredentialEntry::Stored { value: key });
    store.save(&state)
}

async fn handle_one_shot(
    mut stream: UnixStream,
    request: Request,
    shutdown: Arc<Shutdown>,
    started_at: Instant,
) -> anyhow::Result<()> {
    let response = handle_request(&request, started_at).await;
    let frame = encode_frame(&response)?;
    timeout(PEER_REQUEST_TIMEOUT, stream.write_all(&frame))
        .await
        .context("client write timed out")??;

    if matches!(request, Request::Shutdown) {
        shutdown.signal();
    }

    Ok(())
}

/// The production run host: what a run refuses before it starts, and what it does once it will.
struct RealRunHost;

impl super::RunHost for RealRunHost {
    type Prepared = crate::run::PreparedRun;

    async fn prepare(
        &self,
        run_id: &str,
        args: &lns_ipc::RunImageArgs,
    ) -> anyhow::Result<Self::Prepared> {
        crate::run::prepare(run_id, args).await
    }

    async fn serve<W>(
        &self,
        stream: &mut W,
        run_id: String,
        args: lns_ipc::RunImageArgs,
        prepared: Self::Prepared,
    ) -> anyhow::Result<()>
    where
        W: AsyncWriteExt + Unpin + Send,
    {
        serve_prepared_run(stream, run_id, args, prepared).await
    }
}

async fn handle_run(mut stream: UnixStream, args: lns_ipc::RunImageArgs) -> anyhow::Result<()> {
    super::start_run(&mut stream, args, &RealRunHost).await
}

const FRAME_CHAN_BUF: usize = 512;

/// Everything a run does once it is going to start: its registry entry, the `RunStarted` its client waits for, and the boot task's frames.
async fn rebuild_stopped_runs() {
    let records = match crate::cache::root() {
        Ok(root) => crate::run_record::load_all_with(&crate::image_store::RealFs, &root).await,
        Err(e) => Err(e),
    };
    match records {
        Ok(records) => crate::run_registry::rebuild_from_records(records),
        Err(e) => log::warn!("stopped runs not rebuilt; they stay invisible until restart: {e:#}"),
    }
}

async fn serve_prepared_run<W>(
    stream: &mut W,
    run_id: String,
    args: lns_ipc::RunImageArgs,
    prepared: crate::run::PreparedRun,
) -> anyhow::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let (frame_tx, mut frame_rx) = mpsc::channel::<WireFrame>(FRAME_CHAN_BUF);
    let (cancel_tx, cancel_rx) = oneshot::channel::<i32>();
    let (detach_tx, detach_rx) = oneshot::channel::<()>();

    let detached = args.detached;

    let (input_tx, input_rx) = mpsc::channel::<crate::vm::session_client::SessionInput>(256);

    let image_label = args
        .image
        .clone()
        .unwrap_or_else(|| "<imageless>".to_string());
    let command_label = args.cmd.join(" ");
    let started_label = rfc3339_now();
    let requested_name = args.name.clone();
    let config = lns_ipc::RunConfig::from_run_args(&args);

    let logs = Arc::new(crate::run_log::RunLogBuffer::default());
    let (task_frame_tx, tee_rx) = mpsc::channel::<WireFrame>(FRAME_CHAN_BUF);
    tokio::spawn(crate::run_log::tee_frames(
        tee_rx,
        logs.clone(),
        frame_tx.clone(),
    ));
    let run_args = args;
    let task_run_id = run_id.clone();
    let (microvm_tx, microvm_rx) = oneshot::channel::<String>();
    let fallback_microvm = run_id.clone();
    let run_task = tokio::spawn(async move {
        let microvm = microvm_rx.await.unwrap_or(fallback_microvm);
        crate::run::handle(
            task_run_id,
            microvm,
            run_args,
            prepared,
            task_frame_tx,
            input_rx,
        )
        .await;
    });

    let abort = run_task.abort_handle();
    let runtime_cache_registration = crate::image_store::lock_runtime_cache_shared().await;
    let registered = crate::run_registry::register_named(
        run_id.clone(),
        requested_name,
        crate::run_registry::RunHandle {
            cancel_tx,
            detach_tx: std::sync::Mutex::new(Some(detach_tx)),
            task: run_task,
            input_tx: Some(input_tx),
            connector: None,
            name: String::new(),
            image: image_label,
            command: command_label,
            started: started_label,
            status: std::sync::Mutex::new(lns_ipc::RunStatus::Running),
            logs,
            config,
            exec_environment: Default::default(),
        },
    );
    drop(runtime_cache_registration);
    match registered {
        Ok(microvm) => {
            let _ = microvm_tx.send(microvm);
        }
        Err(message) => {
            abort.abort();
            let _ = write_error(stream, message).await;
            return Ok(());
        }
    }

    drop(frame_tx);

    let handshake: anyhow::Result<()> = async {
        let started_frame = encode_frame(&Response::RunStarted {
            run_id: run_id.clone(),
        })
        .context("encoding RunStarted frame")?;
        match timeout(PEER_REQUEST_TIMEOUT, stream.write_all(&started_frame)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(anyhow::Error::from(e).context("writing RunStarted frame")),
            Err(elapsed) => Err(anyhow::Error::from(elapsed).context("writing RunStarted frame")),
        }
    }
    .await;
    if let Err(e) = handshake {
        crate::run_registry::cancel(&run_id);
        crate::run_registry::deregister(&run_id);
        return Err(e);
    }

    let outcome = pump_responses(stream, &mut frame_rx, cancel_rx, detach_rx).await?;
    match post_pump_action(&outcome, detached) {
        PostPumpAction::Retain => {
            crate::run_registry::mark_exited_from_log(&run_id);
        }
        PostPumpAction::BackgroundDrain => {
            if let PumpOutcome::WriteFailed(e) = &outcome {
                log::debug!(run_id = %run_id, error = %e, "detached: CLI closed stream, draining in background");
            }
            tokio::spawn(async move {
                while frame_rx.recv().await.is_some() {}
                crate::run_registry::mark_exited_from_log(&run_id);
            });
        }
        PostPumpAction::CancelAndDeregister => {
            if let PumpOutcome::WriteFailed(e) = &outcome {
                log::debug!(run_id = %run_id, error = %e, "ipc run stream write failed; cancelling run");
            }
            crate::run_registry::cancel(&run_id);
            crate::run_registry::deregister(&run_id);
        }
    }
    Ok(())
}

async fn handle_exec(mut stream: UnixStream, args: lns_ipc::ExecImageArgs) -> anyhow::Result<()> {
    let target_run_id = match crate::run_registry::resolve(&args.run) {
        Ok(id) => id,
        Err(message) => {
            let _ = write_error(&mut stream, message).await;
            return Ok(());
        }
    };
    let Some(connector) = crate::run_registry::connector(&target_run_id) else {
        let _ = write_error(
            &mut stream,
            format!("no active run with id {target_run_id}"),
        )
        .await;
        return Ok(());
    };

    if let Err(message) = validate_exec(&args) {
        let _ = write_error(&mut stream, message).await;
        return Ok(());
    }

    let fd = match connector
        .connect(lns_session::BROKER_PORT, std::time::Duration::from_secs(15))
        .await
    {
        Ok(fd) => fd,
        Err(e) => {
            let _ = write_error(&mut stream, format!("opening exec vsock to broker: {e:#}")).await;
            return Ok(());
        }
    };

    let started_frame = encode_frame(&Response::RunStarted {
        run_id: target_run_id.clone(),
    })
    .context("encoding RunStarted frame")?;
    if let Err(e) = stream.write_all(&started_frame).await {
        // SAFETY: fd was just taken from the only owner; we drop it here.
        unsafe { libc::close(fd) };
        return Err(anyhow::Error::from(e).context("writing exec RunStarted frame"));
    }

    let (frame_tx, mut frame_rx) = mpsc::channel::<WireFrame>(FRAME_CHAN_BUF);
    let (input_tx, input_rx) = mpsc::channel::<crate::vm::session_client::SessionInput>(256);

    let params = build_session_params(args, &target_run_id);

    let session_frame_tx = frame_tx.clone();
    let session_task = tokio::spawn(async move {
        let code = match crate::vm::session_client::run_session_on_fd(
            fd,
            params,
            session_frame_tx.clone(),
            input_rx,
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                let _ = session_frame_tx
                    .send(WireFrame::Json(Response::RunLog {
                        level: lns_ipc::LogLevel::Error,
                        verb: None,
                        message: format!("exec session error: {e:#}"),
                    }))
                    .await;
                1
            }
        };
        let _ = session_frame_tx
            .send(WireFrame::Json(Response::RunExit { code }))
            .await;
    });

    let input_keepalive = input_tx;

    drop(frame_tx);

    let (_dead_cancel_tx, dead_cancel_rx) = oneshot::channel::<i32>();
    let (_dead_detach_tx, dead_detach_rx) = oneshot::channel::<()>();

    let outcome =
        pump_responses(&mut stream, &mut frame_rx, dead_cancel_rx, dead_detach_rx).await?;
    if let PumpOutcome::WriteFailed(e) = &outcome {
        log::debug!(error = %e, "exec stream write failed; tearing session down");
    }

    drop(input_keepalive);
    session_task.abort();
    let _ = session_task.await;
    Ok(())
}
