use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::mpsc;

use lens_sandbox_core::activity::ActivityStream;
use lens_sandbox_core::child_spawner::{self, ChildSpec};
use lens_sandbox_core::client::{SandboxDispatcher, SessionHandler};
use lens_sandbox_core::exec_manager::ExecManager;
use lens_sandbox_core::exec_protocol::IncomingMessage;
use lens_sandbox_core::lifecycle::OrphanReaper;
use lens_sandbox_core::privilege::SandboxCredentials;

use super::runtime::RealAgentRunner;
use super::{DIM, RED, RESET};
use crate::config::AgentConfig;

/// Spawns the agent workload for a started session.
pub(crate) trait AgentRunner: Send + Sync {
    fn spawn_run(
        &self,
        config: Arc<AgentConfig>,
        creds: Option<SandboxCredentials>,
        env: HashMap<String, String>,
        activity: ActivityStream,
        reaper: Arc<OrphanReaper>,
    );
}

pub(crate) struct AgentDispatcher {
    pub config: Arc<AgentConfig>,
    sandbox_creds: Option<SandboxCredentials>,
    reaper: Arc<OrphanReaper>,
    /// Dispatcher-scoped (not per-session) so a WS reconnect can't launch a second agent.
    agent_started: Arc<AtomicBool>,
    /// Observable spawn counter for tests.
    spawn_count: Arc<AtomicUsize>,
    /// Shared across WS sessions so an exec started in one session stays addressable after a reconnect.
    exec_manager: ExecManager,
    /// Shared across WS sessions so the running agent's output reaches the current session after a reconnect.
    activity: ActivityStream,
    runner: Arc<dyn AgentRunner>,
    steps: Arc<dyn crate::scripts::StepRunner>,
    passwd: Arc<dyn crate::scripts::ids::Passwd>,
    abort: Arc<dyn crate::scripts::Abort>,
    /// Held only so a test can await the launch it started; production drops the handle, which detaches the task.
    #[cfg(test)]
    launch: LaunchSlot,
}

#[cfg(test)]
type LaunchSlot = Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>;

impl AgentDispatcher {
    pub fn new(
        config: Arc<AgentConfig>,
        sandbox_creds: Option<SandboxCredentials>,
        reaper: Arc<OrphanReaper>,
    ) -> Self {
        Self::with_runner(
            config,
            sandbox_creds,
            reaper,
            Arc::new(RealAgentRunner),
            Arc::new(super::runtime::RealStepRunner),
            Arc::new(crate::scripts::ids::GuestPasswd),
            Arc::new(super::runtime::ExitBeforeWorkload),
        )
    }

    pub(crate) fn with_runner(
        config: Arc<AgentConfig>,
        sandbox_creds: Option<SandboxCredentials>,
        reaper: Arc<OrphanReaper>,
        runner: Arc<dyn AgentRunner>,
        steps: Arc<dyn crate::scripts::StepRunner>,
        passwd: Arc<dyn crate::scripts::ids::Passwd>,
        abort: Arc<dyn crate::scripts::Abort>,
    ) -> Self {
        let exec_manager = ExecManager::new(
            crate::run_as::setuid_creds(sandbox_creds.as_ref()),
            config.core.is_root,
            reaper.guard(),
        );
        let activity = ActivityStream::new();
        spawn_activity_to_stdout(&activity);
        Self {
            config,
            sandbox_creds,
            reaper,
            agent_started: Arc::new(AtomicBool::new(false)),
            spawn_count: Arc::new(AtomicUsize::new(0)),
            exec_manager,
            activity,
            runner,
            steps,
            passwd,
            abort,
            #[cfg(test)]
            launch: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Await the launch a policy frame started, when it went to a task; a run with no scripts launches inline and has nothing to wait for.
    #[cfg(test)]
    pub(crate) async fn await_launch(&self) {
        let launch = self
            .launch
            .lock()
            .expect("the launch slot is uncontended")
            .take();
        if let Some(handle) = launch {
            let _ = handle.await;
        }
    }

    #[cfg(test)]
    pub(crate) fn spawn_count(&self) -> usize {
        self.spawn_count.load(Ordering::SeqCst)
    }
}

impl SandboxDispatcher for AgentDispatcher {
    fn new_session(&self) -> Box<dyn SessionHandler> {
        Box::new(AgentSession {
            config: self.config.clone(),
            sandbox_creds: self.sandbox_creds.clone(),
            reaper: self.reaper.clone(),
            agent_started: self.agent_started.clone(),
            spawn_count: self.spawn_count.clone(),
            activity: self.activity.clone(),
            exec_manager: self.exec_manager.clone(),
            runner: self.runner.clone(),
            steps: self.steps.clone(),
            passwd: self.passwd.clone(),
            abort: self.abort.clone(),
            #[cfg(test)]
            launch: self.launch.clone(),
        })
    }
}

/// Forward each activity event's summary to `writer`, exiting when the broadcast closes or the writer errors.
pub(crate) fn spawn_activity_forwarder<W>(
    activity: &ActivityStream,
    mut writer: W,
) -> tokio::task::JoinHandle<()>
where
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let mut rx = activity.subscribe();
    tokio::spawn(async move {
        loop {
            let bytes = match rx.recv().await {
                Ok(event) => event.summary.into_bytes(),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    format!("\x1b[90m[...{n} events skipped...]\x1b[0m\r\n").into_bytes()
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            if let Err(e) = writer.write_all(&bytes).await {
                tracing::warn!("activity forwarder write failed: {e}");
                break;
            }
            if let Err(e) = writer.flush().await {
                tracing::warn!("activity forwarder flush failed: {e}");
                break;
            }
        }
    })
}

fn spawn_activity_to_stdout(activity: &ActivityStream) {
    spawn_activity_forwarder(activity, tokio::io::stdout());
}

struct AgentSession {
    config: Arc<AgentConfig>,
    sandbox_creds: Option<SandboxCredentials>,
    reaper: Arc<OrphanReaper>,
    /// Cloned from the dispatcher — shared across reconnect-created sessions.
    agent_started: Arc<AtomicBool>,
    spawn_count: Arc<AtomicUsize>,
    activity: ActivityStream,
    exec_manager: ExecManager,
    runner: Arc<dyn AgentRunner>,
    steps: Arc<dyn crate::scripts::StepRunner>,
    passwd: Arc<dyn crate::scripts::ids::Passwd>,
    abort: Arc<dyn crate::scripts::Abort>,
    #[cfg(test)]
    launch: LaunchSlot,
}

#[async_trait]
impl SessionHandler for AgentSession {
    fn activity(&self) -> &ActivityStream {
        &self.activity
    }

    async fn dispatch(&self, raw_text: &str, tx: &mpsc::UnboundedSender<String>) {
        match serde_json::from_str::<IncomingMessage>(raw_text) {
            Ok(mut msg) => {
                if let IncomingMessage::ExecAttach { env, .. } = &mut msg {
                    let caller = std::mem::take(env);
                    *env = build_agent_env(&self.config, self.sandbox_creds.as_ref(), &caller);
                }
                self.exec_manager.handle(msg, tx).await
            }
            Err(e) => {
                // Not a known exec message — leave for future protocol surfaces.
                tracing::debug!("ignoring non-exec dispatch message: {e}");
            }
        }
    }

    async fn on_policy(&self, env: HashMap<String, String>) {
        if self.agent_started.swap(true, Ordering::SeqCst) {
            tracing::debug!("agent already started, ignoring duplicate policy");
            return;
        }

        let config = self.config.clone();
        let creds = self.sandbox_creds.clone();
        let activity = self.activity.clone();
        let reaper = self.reaper.clone();
        let runner = self.runner.clone();
        let steps = self.steps.clone();
        let passwd = self.passwd.clone();
        let abort = self.abort.clone();
        if config.scripts.is_empty() {
            self.spawn_count.fetch_add(1, Ordering::SeqCst);
            runner.spawn_run(config, creds, env, activity, reaper);
            return;
        }
        let spawned = self.spawn_count.clone();
        // Spawned rather than awaited: a script's own traffic can raise an approval card, and only the read loop that called us can deliver the answer.
        let launch = tokio::spawn(async move {
            if let Err(failure) = run_scripts_before(
                &config,
                creds.as_ref(),
                &env,
                passwd.as_ref(),
                steps.as_ref(),
                &activity,
            )
            .await
            {
                activity.emit(format!("{RED}[scripts]{RESET} {failure}\r\n"));
                abort.refuse(&failure).await;
                return;
            }
            spawned.fetch_add(1, Ordering::SeqCst);
            runner.spawn_run(config, creds, env, activity, reaper);
        });
        #[cfg(test)]
        {
            *self.launch.lock().expect("the launch slot is uncontended") = Some(launch);
        }
        #[cfg(not(test))]
        drop(launch);
    }

    async fn shutdown(&self) {
        // no-op: the agent outlives WS reconnects and exits via process::exit, terminating the supervisor.
    }
}

/// What the agent process gets for its stdio.
#[derive(Debug, PartialEq)]
pub(crate) enum AgentIo {
    Pty,
    Pipes,
}

/// The broker hands this supervisor the PTY slave for a run that asked for a terminal and pipes for one that did not, so our own stdio is the run's `-t` answer: a workload that could detect a terminal the run declined stalls the run on the first pager it spawns.
pub(crate) fn agent_io(session_has_terminal: bool, devpts_mounted: bool) -> AgentIo {
    if session_has_terminal && devpts_mounted {
        AgentIo::Pty
    } else {
        AgentIo::Pipes
    }
}

/// Layer the agent env (parent → project → proxy → creds) then scrub internal vars; CA env is applied later at the Command level.
fn build_agent_env(
    config: &AgentConfig,
    creds: Option<&SandboxCredentials>,
    env: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut full_env: HashMap<String, String> = std::env::vars().collect();
    full_env.extend(env.clone());

    if config.core.is_root {
        let local_proxy = format!("http://{}", config.core.proxy_listen_addr);
        full_env.insert("HTTPS_PROXY".into(), local_proxy.clone());
        full_env.insert("https_proxy".into(), local_proxy.clone());
        full_env.insert("HTTP_PROXY".into(), local_proxy.clone());
        full_env.insert("http_proxy".into(), local_proxy);
    }

    if let Some(home) = effective_home(env, creds) {
        full_env.insert("HOME".into(), home);
    }
    if let Some(user) = effective_value(env, DECLARED_USER, creds.map(SandboxCredentials::user)) {
        full_env.insert("USER".into(), user);
    }

    // Scrub after all layering so no source can reintroduce internal vars
    lens_sandbox_core::privilege::scrub_internal_vars(&mut full_env);

    full_env
}

const DECLARED_HOME: &str = "LENS_SANDBOX_WORKLOAD_HOME";
const DECLARED_USER: &str = "LENS_SANDBOX_WORKLOAD_USER";

fn effective_home(
    env: &HashMap<String, String>,
    creds: Option<&SandboxCredentials>,
) -> Option<String> {
    effective_value(env, DECLARED_HOME, creds.map(SandboxCredentials::home))
}

/// The service delivers the marker through the process env the broker execs us with, not the policy frame, so both sources are read before the scrub drops the prefix.
fn effective_value(
    env: &HashMap<String, String>,
    declared_key: &str,
    from_creds: Option<&str>,
) -> Option<String> {
    env.get(declared_key)
        .cloned()
        .or_else(|| std::env::var(declared_key).ok())
        .filter(|declared| !declared.is_empty())
        .or_else(|| from_creds.map(str::to_string))
}

/// Resolve the agent cwd: explicit `WORKSPACE_PATH`, else the post-setuid home, else ".".
fn resolve_cwd(workspace_path: Option<&str>, creds_home: Option<&str>) -> String {
    if let Some(w) = workspace_path.filter(|s| !s.is_empty()) {
        return w.to_string();
    }
    if let Some(h) = creds_home.filter(|s| !s.is_empty()) {
        return h.to_string();
    }
    ".".to_string()
}

/// A script gets the layering the workload gets, so a step reaching a mirror sees the same proxy and CA env; without it a TLS failure would look like a broken package index.
pub(crate) fn build_script_env(
    config: &AgentConfig,
    creds: Option<&SandboxCredentials>,
    env: &HashMap<String, String>,
) -> HashMap<String, String> {
    build_agent_env(config, creds, env)
}

/// A script runs in its own user's home, not the workload's workdir: the workdir may be a bind the script is meant to prepare rather than sit inside.
pub(crate) fn resolve_script_cwd(creds: Option<&SandboxCredentials>) -> String {
    creds
        .map(SandboxCredentials::home)
        .filter(|home| !home.is_empty())
        .unwrap_or("/")
        .to_string()
}

async fn run_scripts_before(
    config: &AgentConfig,
    creds: Option<&SandboxCredentials>,
    env: &HashMap<String, String>,
    passwd: &dyn crate::scripts::ids::Passwd,
    steps: &dyn crate::scripts::StepRunner,
    activity: &ActivityStream,
) -> Result<(), crate::scripts::ScriptFailure> {
    let scripts = crate::scripts::prepare(config, creds, env, passwd)?;
    crate::scripts::run_all(&scripts, steps, activity).await
}

/// Assemble the agent `ChildSpec` with agent-specific env layering atop the shared `child_spawner` hardening.
pub(crate) fn agent_child_spec(
    config: &AgentConfig,
    creds: Option<&SandboxCredentials>,
    env: &HashMap<String, String>,
) -> ChildSpec {
    ChildSpec {
        argv: vec!["sh".into(), "-c".into(), config.agent_command.clone()],
        cwd: Some(resolve_cwd(
            config.workspace_path.as_deref(),
            effective_home(env, creds).as_deref(),
        )),
        env: build_agent_env(config, creds, env),
        creds: crate::run_as::setuid_creds(creds),
        is_root: config.core.is_root,
    }
}

pub(crate) fn build_agent_command(
    config: &AgentConfig,
    creds: Option<&SandboxCredentials>,
    env: &HashMap<String, String>,
) -> Command {
    child_spawner::build_command(&agent_child_spec(config, creds, env))
}

/// Relay a headless workload's stream as it arrives: its bytes are the run's output, so nothing here prefixes, splits, or re-terminates them.
pub(crate) async fn relay_verbatim(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    activity: ActivityStream,
) {
    let mut chunk = vec![0u8; 8192];
    loop {
        let n = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        activity.emit(String::from_utf8_lossy(&chunk[..n]).into_owned());
    }
}

const MAX_LINE_LEN: usize = 4096;

/// Emit one activity event per newline, capping each line at `MAX_LINE_LEN` bytes so an agent can't force unbounded allocation with a newline-less line.
pub(crate) async fn stream_output(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    activity: ActivityStream,
    prefix: &str,
    color: &str,
) {
    let mut line: Vec<u8> = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 4096];
    loop {
        let n = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut rest = &chunk[..n];
        while let Some(pos) = rest.iter().position(|&b| b == b'\n') {
            extend_capped(&mut line, &rest[..pos], &mut truncated);
            emit_output_line(&activity, &line, truncated, prefix, color);
            line.clear();
            truncated = false;
            rest = &rest[pos + 1..];
        }
        extend_capped(&mut line, rest, &mut truncated);
    }
    if !line.is_empty() {
        emit_output_line(&activity, &line, truncated, prefix, color);
    }
}

fn extend_capped(line: &mut Vec<u8>, more: &[u8], truncated: &mut bool) {
    let take = more.len().min(MAX_LINE_LEN.saturating_sub(line.len()));
    line.extend_from_slice(&more[..take]);
    if take < more.len() {
        *truncated = true;
    }
}

fn emit_output_line(
    activity: &ActivityStream,
    line: &[u8],
    truncated: bool,
    prefix: &str,
    color: &str,
) {
    let text = String::from_utf8_lossy(line);
    if truncated {
        activity.emit(format!(
            "{color}{prefix}{RESET} {text}…{DIM}[truncated]{RESET}\r\n"
        ));
    } else {
        activity.emit(format!("{color}{prefix}{RESET} {text}\r\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use lens_sandbox_core::ca_env::CA_BUNDLE;
    use lens_sandbox_core::client::SandboxDispatcher;
    use lens_sandbox_core::config::{
        CoreConfig, DEFAULT_DNS_STUB_PORT, DEFAULT_PROXY_PORT, DEFAULT_TRANSPARENT_PORT,
        SandboxMode,
    };
    use tokio::sync::mpsc;

    const PRINT_CA_ENV_CMD: &str = "printf '%s|%s|%s|%s|%s' \"$SSL_CERT_FILE\" \"$REQUESTS_CA_BUNDLE\" \"$NODE_EXTRA_CA_CERTS\" \"$CURL_CA_BUNDLE\" \"$GIT_SSL_CAINFO\"";

    struct FakeRunner {
        calls: Arc<AtomicUsize>,
    }

    impl AgentRunner for FakeRunner {
        fn spawn_run(
            &self,
            _config: Arc<AgentConfig>,
            _creds: Option<SandboxCredentials>,
            _env: HashMap<String, String>,
            _activity: ActivityStream,
            _reaper: Arc<OrphanReaper>,
        ) {
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn make_agent_config() -> AgentConfig {
        AgentConfig {
            core: CoreConfig {
                mode: SandboxMode::Server {
                    ws_url: String::new(),
                    token: String::new(),
                },
                idle_timeout: None,
                proxy_listen_addr: ([127, 0, 0, 1], DEFAULT_PROXY_PORT).into(),
                transparent_listen_addr: ([127, 0, 0, 1], DEFAULT_TRANSPARENT_PORT).into(),
                dns_stub_listen_addr: ([127, 0, 0, 1], DEFAULT_DNS_STUB_PORT).into(),
                is_root: false,
                sandbox_user: "sandbox".into(),
            },
            agent_command: "echo hello".into(),
            workspace_path: Some("/tmp".into()),
            scripts: Vec::new(),
        }
    }

    #[test]
    fn a_run_that_declined_a_terminal_gets_pipes_even_where_devpts_is_mounted() {
        assert_eq!(
            agent_io(false, true),
            AgentIo::Pipes,
            "the broker gave us pipes because the run asked for no terminal; a PTY here lets the workload detect one and a pager stalls the run forever"
        );
    }

    #[test]
    fn a_run_that_asked_for_a_terminal_gets_a_pty() {
        assert_eq!(agent_io(true, true), AgentIo::Pty);
    }

    #[test]
    fn a_terminal_run_falls_back_to_pipes_without_devpts() {
        assert_eq!(
            agent_io(true, false),
            AgentIo::Pipes,
            "openpty cannot allocate a pair without devpts, so the agent must still start"
        );
    }

    #[test]
    fn resolve_cwd_prefers_explicit_workspace_path() {
        assert_eq!(resolve_cwd(Some("/app"), Some("/home/node")), "/app");
    }

    #[test]
    fn resolve_cwd_falls_back_to_creds_home() {
        assert_eq!(resolve_cwd(None, Some("/home/node")), "/home/node");
    }

    #[test]
    fn resolve_cwd_treats_empty_strings_as_absent() {
        assert_eq!(resolve_cwd(Some(""), Some("/home/node")), "/home/node");
    }

    #[test]
    fn resolve_cwd_final_fallback_is_dot() {
        assert_eq!(resolve_cwd(None, None), ".");
    }

    #[test]
    #[serial_test::serial(env)]
    fn build_agent_env_inserts_home_and_user_from_creds() {
        // The Some(creds) arm must layer HOME/USER and survive the post-layering scrub.
        let creds = SandboxCredentials::resolve_by_uid(0, 0).expect("uid 0 resolves on host");
        let config = make_agent_config();
        let env = build_agent_env(&config, Some(&creds), &HashMap::new());
        assert_eq!(env.get("HOME").map(String::as_str), Some(creds.home()));
        assert_eq!(env.get("USER").map(String::as_str), Some(creds.user()));
    }

    #[test]
    #[serial_test::serial(env)]
    fn a_declared_home_outranks_the_run_as_users_passwd_home() {
        let creds = SandboxCredentials::resolve_by_uid(0, 0).expect("uid 0 resolves on host");
        let config = make_agent_config();
        let declared = HashMap::from([
            (
                "LENS_SANDBOX_WORKLOAD_HOME".to_string(),
                "/home/sandbox".to_string(),
            ),
            (
                "LENS_SANDBOX_WORKLOAD_USER".to_string(),
                "builder".to_string(),
            ),
        ]);

        let env = build_agent_env(&config, Some(&creds), &declared);

        assert_eq!(env.get("HOME").map(String::as_str), Some("/home/sandbox"));
        assert_eq!(env.get("USER").map(String::as_str), Some("builder"));
        assert!(
            !env.keys().any(|k| k.starts_with("LENS_SANDBOX_")),
            "the marker must never reach the workload"
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn a_declared_home_is_also_the_agent_cwd_fallback() {
        let creds = SandboxCredentials::resolve_by_uid(0, 0).expect("uid 0 resolves on host");
        let mut config = make_agent_config();
        config.workspace_path = None;
        let declared = HashMap::from([(
            "LENS_SANDBOX_WORKLOAD_HOME".to_string(),
            "/home/sandbox".to_string(),
        )]);

        let spec = agent_child_spec(&config, Some(&creds), &declared);

        assert_eq!(spec.cwd.as_deref(), Some("/home/sandbox"));
    }

    #[test]
    #[serial_test::serial(env)]
    fn the_marker_is_read_from_the_env_the_broker_execs_us_with() {
        // SAFETY: serialized via #[serial(env)]; no other thread reads env during this body.
        unsafe {
            std::env::set_var("LENS_SANDBOX_WORKLOAD_HOME", "/home/sandbox");
        }
        let creds = SandboxCredentials::resolve_by_uid(0, 0).expect("uid 0 resolves on host");
        let mut config = make_agent_config();
        config.workspace_path = None;

        let spec = agent_child_spec(&config, Some(&creds), &HashMap::new());

        // SAFETY: serialized via #[serial(env)]; no other thread reads env during this body.
        unsafe {
            std::env::remove_var("LENS_SANDBOX_WORKLOAD_HOME");
        }
        assert_eq!(
            spec.env.get("HOME").map(String::as_str),
            Some("/home/sandbox"),
            "the policy frame carries no env, so the inherited env is the only place the marker arrives"
        );
        assert_eq!(spec.cwd.as_deref(), Some("/home/sandbox"));
        assert!(!spec.env.keys().any(|k| k.starts_with("LENS_SANDBOX_")));
    }

    #[test]
    #[serial_test::serial(env)]
    fn the_run_as_users_home_still_fills_an_undeclared_home() {
        let creds = SandboxCredentials::resolve_by_uid(0, 0).expect("uid 0 resolves on host");
        let config = make_agent_config();

        let env = build_agent_env(&config, Some(&creds), &HashMap::new());

        assert_eq!(env.get("HOME").map(String::as_str), Some(creds.home()));
        assert_eq!(env.get("USER").map(String::as_str), Some(creds.user()));
    }

    #[test]
    #[serial_test::serial(env)]
    fn a_root_run_as_keeps_its_identity_but_not_its_setuid() {
        let creds = SandboxCredentials::resolve_by_uid(0, 0).expect("uid 0 resolves on host");
        let mut config = make_agent_config();
        config.workspace_path = None;

        let spec = agent_child_spec(&config, Some(&creds), &HashMap::new());

        assert!(
            spec.creds.is_none(),
            "a root run-as must reach child_spawner's cap-drop branch, which only runs when creds is None"
        );
        assert_eq!(
            spec.env.get("HOME").map(String::as_str),
            Some(creds.home()),
            "dropping the setuid must not cost the workload its HOME — the flag exists so root-owned tooling works"
        );
        assert_eq!(spec.env.get("USER").map(String::as_str), Some(creds.user()));
        assert_eq!(
            spec.cwd.as_deref(),
            Some(creds.home()),
            "with no explicit workspace the agent still starts in the run-as user's home"
        );
    }

    #[test]
    fn a_non_root_run_as_still_setuids() {
        let creds = SandboxCredentials::resolve_by_uid(65534, 65534).expect("uid resolves on host");
        let config = make_agent_config();

        let spec = agent_child_spec(&config, Some(&creds), &HashMap::new());

        let (uid, gid) = spec.creds.expect("a non-root run-as setuids").uid_gid();
        assert_eq!((uid.as_raw(), gid.as_raw()), (65534, 65534));
    }

    #[tokio::test]
    async fn dispatch_ignores_all_messages() {
        let config = Arc::new(make_agent_config());
        let dispatcher = AgentDispatcher::new(config, None, Arc::new(OrphanReaper::spawn()));
        let session = dispatcher.new_session();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();

        session.dispatch("not valid json {{{", &tx).await;
        session
            .dispatch(r#"{"type":"exec","id":"r1","command":"ls"}"#, &tx)
            .await;

        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn session_activity_exposes_its_stream() {
        let config = Arc::new(make_agent_config());
        let dispatcher = AgentDispatcher::new(config, None, Arc::new(OrphanReaper::spawn()));
        let session = dispatcher.new_session();

        let mut rx = session.activity().subscribe();
        session.activity().emit("ping\n".to_string());
        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("event within 1s")
            .expect("activity sender open");
        assert_eq!(event.summary, "ping\n");
    }

    #[tokio::test]
    async fn activity_is_shared_across_reconnect_sessions() {
        let config = Arc::new(make_agent_config());
        let dispatcher = AgentDispatcher::new(config, None, Arc::new(OrphanReaper::spawn()));
        let session1 = dispatcher.new_session();
        let session2 = dispatcher.new_session();

        let mut rx2 = session2.activity().subscribe();
        session1.activity().emit("post-reconnect\n".to_string());
        let event = tokio::time::timeout(Duration::from_secs(1), rx2.recv())
            .await
            .expect("event within 1s")
            .expect("activity sender open");
        assert_eq!(event.summary, "post-reconnect\n");
    }

    #[tokio::test]
    async fn shutdown_leaves_the_agent_running() {
        // shutdown is a no-op: the agent outlives the WS session, so it must not tear anything down.
        let config = Arc::new(make_agent_config());
        let dispatcher = AgentDispatcher::new(config, None, Arc::new(OrphanReaper::spawn()));
        let session = dispatcher.new_session();
        session.shutdown().await;
    }

    #[tokio::test]
    async fn on_policy_runs_agent_once_across_reconnects() {
        let calls = Arc::new(AtomicUsize::new(0));
        let runner = Arc::new(FakeRunner {
            calls: calls.clone(),
        });
        let dispatcher = AgentDispatcher::with_runner(
            Arc::new(make_agent_config()),
            None,
            Arc::new(OrphanReaper::spawn()),
            runner,
            Arc::new(NoScripts),
            Arc::new(NoPasswd),
            Arc::new(RecordingAbort::default()),
        );

        dispatcher.new_session().on_policy(HashMap::new()).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "first policy spawns once");
        assert_eq!(dispatcher.spawn_count(), 1);

        // Simulate WS reconnect: a fresh session must not re-spawn.
        dispatcher.new_session().on_policy(HashMap::new()).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "reconnect must not spawn a second agent",
        );
        assert_eq!(dispatcher.spawn_count(), 1);
    }

    #[tokio::test]
    async fn exec_manager_persists_across_reconnects() {
        // The exec registry must outlive a WS session: session2 reattaching session1's id hits "already in use".
        let mut config = make_agent_config();
        config.agent_command = "true".into();
        let dispatcher =
            AgentDispatcher::new(Arc::new(config), None, Arc::new(OrphanReaper::spawn()));

        let session1 = dispatcher.new_session();
        let (tx1, mut rx1) = mpsc::unbounded_channel::<String>();

        let attach = r#"{"type":"exec_attach","execId":"e1","argv":["sleep","30"],"env":{},"tty":false,"stdin":true,"stdout":true,"stderr":true}"#;
        session1.dispatch(attach, &tx1).await;

        // Drain exec_attached so we know the exec is registered.
        let frame = tokio::time::timeout(Duration::from_secs(5), rx1.recv())
            .await
            .expect("attached within 5s")
            .expect("rx1 open");
        let v: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["type"], "exec_attached");

        // Simulate WS reconnect.
        let session2 = dispatcher.new_session();
        let (tx2, mut rx2) = mpsc::unbounded_channel::<String>();

        let dup_attach = r#"{"type":"exec_attach","execId":"e1","argv":["true"],"env":{},"tty":false,"stdin":true,"stdout":true,"stderr":true}"#;
        session2.dispatch(dup_attach, &tx2).await;

        let dup = tokio::time::timeout(Duration::from_secs(5), rx2.recv())
            .await
            .expect("dup error within 5s")
            .expect("rx2 open");
        let dv: serde_json::Value = serde_json::from_str(&dup).unwrap();
        assert_eq!(dv["type"], "exec_error");
        let msg = dv["message"].as_str().unwrap();
        assert!(
            msg.contains("already in use"),
            "expected 'already in use', got: {msg}"
        );

        // Clean up the long-running child so the test process exits.
        let kill = format!(
            r#"{{"type":"exec_cancel","execId":"e1","signal":{}}}"#,
            libc::SIGKILL
        );
        session2.dispatch(&kill, &tx2).await;
        // Read both rx's — we only care that exec_exit appears somewhere within the timeout.
        let mut saw_exit = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !saw_exit && tokio::time::Instant::now() < deadline {
            tokio::select! {
                Some(f) = rx1.recv() => {
                    let v: serde_json::Value = serde_json::from_str(&f).unwrap();
                    if v["type"] == "exec_exit" { saw_exit = true; }
                }
                Some(f) = rx2.recv() => {
                    let v: serde_json::Value = serde_json::from_str(&f).unwrap();
                    if v["type"] == "exec_exit" { saw_exit = true; }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
        assert!(saw_exit, "exec_exit did not arrive within 5s after kill");
    }

    #[test]
    fn build_agent_command_applies_project_env() {
        let config = make_agent_config();
        let mut env = HashMap::new();
        env.insert("GH_TOKEN".into(), "ghp_secret".into());
        env.insert("API_KEY".into(), "key123".into());

        let cmd = build_agent_command(&config, None, &env);
        let envs: HashMap<_, _> = cmd
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
            .collect();

        assert_eq!(envs.get("GH_TOKEN").unwrap(), "ghp_secret");
        assert_eq!(envs.get("API_KEY").unwrap(), "key123");
    }

    #[test]
    fn build_agent_command_proxy_overrides_project_env() {
        let mut config = make_agent_config();
        config.core.is_root = true;

        let mut env = HashMap::new();
        env.insert("HTTPS_PROXY".into(), "http://user-set-proxy".into());

        let cmd = build_agent_command(&config, None, &env);
        let envs: HashMap<_, _> = cmd
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
            .collect();

        assert_eq!(envs.get("HTTPS_PROXY").unwrap(), "http://127.0.0.1:3128");
    }

    #[test]
    fn the_proxy_the_workload_is_pointed_at_is_the_one_an_exec_session_is_pointed_at() {
        let mut config = make_agent_config();
        config.core.is_root = true;

        let env = build_agent_env(&config, None, &HashMap::new());

        assert_eq!(
            env.get("HTTPS_PROXY").map(String::as_str),
            Some(lns_session::GUEST_PROXY_URL),
            "the service points an exec session at this spelling without asking the guest, so a change here that leaves the constant behind sends an exec around the gate"
        );
    }

    #[test]
    fn build_agent_command_ca_env_overrides_project_env() {
        let config = make_agent_config();
        let mut env = HashMap::new();
        env.insert("SSL_CERT_FILE".into(), "/tmp/evil.pem".into());
        env.insert("NODE_EXTRA_CA_CERTS".into(), "/tmp/evil.pem".into());

        let cmd = build_agent_command(&config, None, &env);
        let envs: HashMap<_, _> = cmd
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
            .collect();

        // CA env must win over project env
        assert_eq!(envs.get("SSL_CERT_FILE").unwrap(), CA_BUNDLE);
        assert_eq!(envs.get("NODE_EXTRA_CA_CERTS").unwrap(), CA_BUNDLE);
    }

    #[tokio::test]
    async fn build_agent_command_applies_ca_env() {
        let mut config = make_agent_config();
        config.agent_command = PRINT_CA_ENV_CMD.into();

        let mut cmd = build_agent_command(&config, None, &HashMap::new());
        let output = cmd.output().await.expect("command should run");
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(output.status.success(), "stderr: {stderr}");

        let stdout = String::from_utf8(output.stdout).expect("stdout should be valid UTF-8");
        let expected = format!("{0}|{0}|{0}|{0}|{0}", CA_BUNDLE);
        assert_eq!(stdout, expected);
    }

    #[tokio::test]
    async fn stream_output_emits_lines_and_truncates_overlong() {
        let activity = ActivityStream::new();
        let mut rx = activity.subscribe();

        let long = "x".repeat(MAX_LINE_LEN + 50);
        let input = format!("short line\n{long}\n");
        stream_output(input.as_bytes(), activity.clone(), "[stdout]", DIM).await;
        drop(activity);

        let first = rx.recv().await.expect("first line emitted");
        assert!(
            first.summary.contains("short line"),
            "got: {first:?}",
            first = first.summary
        );

        let second = rx.recv().await.expect("second line emitted");
        assert!(
            second.summary.contains("[truncated]"),
            "overlong line must be truncated: {}",
            second.summary
        );
        assert!(
            second.summary.len() < long.len(),
            "truncated payload must be shorter than the input line"
        );
    }

    #[tokio::test]
    async fn stream_output_bounds_a_newlineless_blob() {
        let activity = ActivityStream::new();
        let mut rx = activity.subscribe();

        let blob = "y".repeat(MAX_LINE_LEN * 8);
        stream_output(blob.as_bytes(), activity.clone(), "[stdout]", DIM).await;
        drop(activity);

        let event = rx
            .recv()
            .await
            .expect("newlineless blob still emits at EOF");
        assert!(
            event.summary.contains("[truncated]"),
            "a line with no newline must be truncated, not buffered whole: {}",
            event.summary
        );
        assert!(
            event.summary.len() < blob.len() / 4,
            "emitted line stays bounded well under the {}-byte input (got {})",
            blob.len(),
            event.summary.len()
        );
    }

    #[tokio::test]
    async fn emit_output_line_terminates_with_crlf() {
        let activity = ActivityStream::new();
        let mut rx = activity.subscribe();

        stream_output("hello\n".as_bytes(), activity.clone(), "[stdout]", DIM).await;
        drop(activity);

        let event = rx.recv().await.expect("line emitted");
        assert!(
            event.summary.ends_with("\r\n"),
            "activity event must end with CRLF, got: {:?}",
            &event.summary[event.summary.len().saturating_sub(4)..]
        );
    }

    #[tokio::test]
    async fn a_headless_workloads_output_is_relayed_byte_for_byte() {
        let activity = ActivityStream::new();
        let (writer, mut reader) = tokio::io::duplex(256);
        spawn_activity_forwarder(&activity, writer);

        relay_verbatim("commit abc\ndiff --git\n".as_bytes(), activity.clone()).await;
        drop(activity);

        let mut relayed = Vec::new();
        reader
            .read_to_end(&mut relayed)
            .await
            .expect("relayed bytes");
        assert_eq!(
            String::from_utf8(relayed).expect("utf8"),
            "commit abc\ndiff --git\n",
            "a headless run relays the workload's own bytes: no stream prefix, no line rewriting"
        );
    }

    #[tokio::test]
    async fn spawn_activity_forwarder_writes_emitted_events() {
        let activity = ActivityStream::new();
        let (writer, mut reader) = tokio::io::duplex(256);
        spawn_activity_forwarder(&activity, writer);

        activity.emit("hello\n".to_string());
        activity.emit("world\n".to_string());

        // Dropping the stream closes the broadcast; the forwarder drains buffered events then EOFs the reader.
        drop(activity);

        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "hello\nworld\n");
    }

    #[tokio::test]
    async fn spawn_activity_forwarder_exits_when_activity_dropped() {
        let activity = ActivityStream::new();
        let (writer, mut reader) = tokio::io::duplex(64);
        spawn_activity_forwarder(&activity, writer);
        drop(activity);

        // With the sender dropped and nothing buffered, the task exits promptly and the reader sees EOF.
        let mut buf = [0u8; 16];
        let read_result = tokio::time::timeout(Duration::from_secs(1), reader.read(&mut buf)).await;
        assert!(read_result.is_ok(), "forwarder task did not exit");
        assert_eq!(read_result.unwrap().unwrap(), 0, "expected EOF");
    }

    #[tokio::test]
    async fn spawn_activity_forwarder_survives_lagged() {
        // Tiny duplex parks the forwarder so the 256-cap broadcast ring overflows before it drains.
        let activity = ActivityStream::new();
        let (writer, mut reader) = tokio::io::duplex(8);
        spawn_activity_forwarder(&activity, writer);

        activity.emit("first\n".to_string());
        tokio::task::yield_now().await;

        // Overflow the 256-cap ring while the forwarder is parked.
        for i in 0..300 {
            activity.emit(format!("burst-{i}\n"));
        }
        // Sentinel after the burst — only delivered if the loop survives Lagged.
        activity.emit("final\n".to_string());
        drop(activity);

        let mut buf = Vec::new();
        let read_result =
            tokio::time::timeout(Duration::from_secs(5), reader.read_to_end(&mut buf)).await;
        assert!(read_result.is_ok(), "forwarder did not drain in time");
        let s = String::from_utf8(buf).unwrap();

        assert!(s.contains("first\n"), "pre-lag event missing: {s:?}");
        assert!(s.contains("events skipped"), "lag marker missing: {s:?}");
        assert!(
            s.ends_with("final\n"),
            "post-lag event missing — forwarder likely exited on Lagged: {s:?}"
        );
    }

    #[tokio::test]
    async fn spawn_activity_forwarder_exits_on_writer_error() {
        let activity = ActivityStream::new();
        let (writer, reader) = tokio::io::duplex(8);
        drop(reader);
        let handle = spawn_activity_forwarder(&activity, writer);

        activity.emit("trigger\n".to_string());

        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("forwarder did not exit on writer error")
            .expect("forwarder task panicked");
    }

    #[tokio::test]
    async fn spawn_activity_forwarder_exits_on_flush_error() {
        struct FailingFlush;
        impl tokio::io::AsyncWrite for FailingFlush {
            fn poll_write(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
                buf: &[u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                std::task::Poll::Ready(Ok(buf.len()))
            }
            fn poll_flush(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Err(std::io::Error::other("flush boom")))
            }
            fn poll_shutdown(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
        }

        use tokio::io::AsyncWriteExt;
        FailingFlush.shutdown().await.unwrap();

        let activity = ActivityStream::new();
        let handle = spawn_activity_forwarder(&activity, FailingFlush);
        activity.emit("trigger\n".to_string());

        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("forwarder did not exit on flush error")
            .expect("forwarder task panicked");
    }

    #[test]
    fn build_agent_env_preserves_policy_env_vars() {
        let config = make_agent_config();
        let mut env = HashMap::new();
        env.insert(
            "MCP_URL".into(),
            "https://lens.example.com/projects/proj-1/mcp".into(),
        );

        let full_env = build_agent_env(&config, None, &env);

        assert_eq!(
            full_env.get("MCP_URL").map(String::as_str),
            Some("https://lens.example.com/projects/proj-1/mcp"),
            "policy-injected env vars must survive layering and scrubbing"
        );

        // And the built command should carry it too.
        let cmd = build_agent_command(&config, None, &env);
        let envs: HashMap<_, _> = cmd
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
            .collect();
        assert_eq!(
            envs.get("MCP_URL").map(String::as_str),
            Some("https://lens.example.com/projects/proj-1/mcp")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn build_agent_env_scrubs_internal_vars() {
        // SAFETY: serialized via #[serial(env)]; no other thread reads env during this body.
        unsafe {
            std::env::set_var("LENS_SANDBOX_POLICY_FILE", "/run/lens-policy/policy.json");
            std::env::set_var("LENS_SANDBOX_TOKEN", "secret-token");
            std::env::set_var("LENS_SANDBOX_WS_URL", "ws://localhost:3002");
            std::env::set_var("AGENT_KEEP_ME", "visible-to-agent");
        }

        let config = make_agent_config();
        let env = build_agent_env(&config, None, &HashMap::new());

        assert!(
            !env.contains_key("LENS_SANDBOX_POLICY_FILE"),
            "LENS_SANDBOX_POLICY_FILE must be scrubbed"
        );
        assert!(
            !env.contains_key("LENS_SANDBOX_TOKEN"),
            "LENS_SANDBOX_TOKEN must be scrubbed"
        );
        assert!(
            !env.contains_key("LENS_SANDBOX_WS_URL"),
            "LENS_SANDBOX_WS_URL must be scrubbed"
        );
        // Inverse: prefix-based scrub must not nuke ordinary agent vars.
        assert_eq!(
            env.get("AGENT_KEEP_ME").map(String::as_str),
            Some("visible-to-agent"),
            "non-internal vars must survive scrubbing"
        );

        // SAFETY: serialized via #[serial(env)]; no other thread reads env during this body.
        unsafe {
            std::env::remove_var("LENS_SANDBOX_POLICY_FILE");
            std::env::remove_var("LENS_SANDBOX_TOKEN");
            std::env::remove_var("LENS_SANDBOX_WS_URL");
            std::env::remove_var("AGENT_KEEP_ME");
        }
    }

    /// A step runner a scenario reaches only when it declares scripts; recording the calls is how a test proves the ordering.
    #[derive(Default)]
    struct RecordingSteps {
        ran: Arc<std::sync::Mutex<Vec<String>>>,
        fail_with: Option<i32>,
    }

    #[async_trait]
    impl crate::scripts::StepRunner for RecordingSteps {
        async fn run(
            &self,
            script: &crate::scripts::PreparedScript,
            _position: &str,
            _activity: ActivityStream,
        ) -> Result<i32, String> {
            self.ran
                .lock()
                .expect("uncontended")
                .push(script.label.clone());
            Ok(self.fail_with.unwrap_or(0))
        }
    }

    struct NoScripts;

    #[async_trait]
    impl crate::scripts::StepRunner for NoScripts {
        async fn run(
            &self,
            _script: &crate::scripts::PreparedScript,
            _position: &str,
            _activity: ActivityStream,
        ) -> Result<i32, String> {
            unreachable!("a run declaring no scripts must never reach a step runner")
        }
    }

    /// Records the refusal instead of exiting, so a test can see that the workload stayed unspawned.
    #[derive(Default)]
    struct RecordingAbort {
        refused: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl crate::scripts::Abort for RecordingAbort {
        async fn refuse(&self, failure: &crate::scripts::ScriptFailure) {
            self.refused
                .lock()
                .expect("uncontended")
                .push(failure.to_string());
        }
    }

    struct NoPasswd;

    impl crate::scripts::ids::Passwd for NoPasswd {
        fn uid_of(&self, _name: &str) -> Option<u32> {
            None
        }
        fn primary_gid_of(&self, _name: &str) -> Option<u32> {
            None
        }
        fn gid_of_group(&self, _group: &str) -> Option<u32> {
            None
        }
    }

    fn config_with_scripts(labels: &[&str]) -> AgentConfig {
        let mut config = make_agent_config();
        config.scripts = labels
            .iter()
            .enumerate()
            .map(|(i, label)| lns_session::ScriptManifestStep {
                script: format!("/.lens/scripts/{i:03}.sh"),
                user: None,
                label: (*label).to_string(),
            })
            .collect();
        config
    }

    #[tokio::test]
    async fn a_run_declaring_no_scripts_reaches_its_workload_exactly_as_before() {
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher = AgentDispatcher::with_runner(
            Arc::new(make_agent_config()),
            None,
            Arc::new(OrphanReaper::spawn()),
            Arc::new(FakeRunner {
                calls: calls.clone(),
            }),
            Arc::new(NoScripts),
            Arc::new(NoPasswd),
            Arc::new(RecordingAbort::default()),
        );
        dispatcher.new_session().on_policy(HashMap::new()).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the block is optional, so the overwhelmingly common run must launch inline with nothing new between the policy frame and the workload"
        );
    }

    #[tokio::test]
    async fn the_scripts_run_in_order_and_the_workload_follows_them() {
        let calls = Arc::new(AtomicUsize::new(0));
        let steps = Arc::new(RecordingSteps::default());
        let dispatcher = AgentDispatcher::with_runner(
            Arc::new(config_with_scripts(&["install psql", "seed the cache"])),
            None,
            Arc::new(OrphanReaper::spawn()),
            Arc::new(FakeRunner {
                calls: calls.clone(),
            }),
            steps.clone(),
            Arc::new(NoPasswd),
            Arc::new(RecordingAbort::default()),
        );
        dispatcher.new_session().on_policy(HashMap::new()).await;
        dispatcher.await_launch().await;
        assert_eq!(
            *steps.ran.lock().expect("uncontended"),
            ["install psql", "seed the cache"],
            "the manifest's order is the merge's order, and the workload is what the scripts prepare for"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the workload still starts once every script succeeded"
        );
    }

    #[tokio::test]
    async fn a_failing_script_means_the_workload_never_starts() {
        let calls = Arc::new(AtomicUsize::new(0));
        let steps = Arc::new(RecordingSteps {
            fail_with: Some(100),
            ..Default::default()
        });
        let abort = Arc::new(RecordingAbort::default());
        let dispatcher = AgentDispatcher::with_runner(
            Arc::new(config_with_scripts(&["install psql"])),
            None,
            Arc::new(OrphanReaper::spawn()),
            Arc::new(FakeRunner {
                calls: calls.clone(),
            }),
            steps.clone(),
            Arc::new(NoPasswd),
            abort.clone(),
        );
        dispatcher.new_session().on_policy(HashMap::new()).await;
        dispatcher.await_launch().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "the scripts are the environment the workload was promised, so a workload started without them would fail in ways nobody could trace back here"
        );
        let refused = abort.refused.lock().expect("uncontended").clone();
        assert_eq!(refused.len(), 1);
        assert!(
            refused[0].contains("install psql") && refused[0].contains("100"),
            "the refusal identifies the script and the status it exited with; got: {:?}",
            refused[0]
        );
    }

    #[tokio::test]
    async fn a_reconnect_does_not_run_the_scripts_a_second_time() {
        let steps = Arc::new(RecordingSteps::default());
        let dispatcher = AgentDispatcher::with_runner(
            Arc::new(config_with_scripts(&["install psql"])),
            None,
            Arc::new(OrphanReaper::spawn()),
            Arc::new(FakeRunner {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            steps.clone(),
            Arc::new(NoPasswd),
            Arc::new(RecordingAbort::default()),
        );
        dispatcher.new_session().on_policy(HashMap::new()).await;
        dispatcher.await_launch().await;
        dispatcher.new_session().on_policy(HashMap::new()).await;
        dispatcher.await_launch().await;
        assert_eq!(
            steps.ran.lock().expect("uncontended").len(),
            1,
            "the latch sits ahead of the spawn, so a dropped websocket cannot make a run install its packages twice"
        );
    }

    #[tokio::test]
    async fn a_script_naming_a_user_the_guest_cannot_resolve_never_reaches_a_step() {
        let mut config = config_with_scripts(&["install psql"]);
        config.scripts[0].user = Some("postgres".into());
        let steps = Arc::new(RecordingSteps::default());
        let failure = run_scripts_before(
            &config,
            None,
            &HashMap::new(),
            &NoPasswd,
            steps.as_ref(),
            &ActivityStream::new(),
        )
        .await
        .expect_err("an unresolvable user refuses the run");
        assert!(
            steps.ran.lock().expect("uncontended").is_empty(),
            "every user resolves before the first script runs, so a run naming an identity this guest lacks fails before it has half-prepared itself"
        );
        assert!(matches!(
            failure,
            crate::scripts::ScriptFailure::UnresolvableUser { .. }
        ));
    }

    #[tokio::test]
    async fn a_script_gets_the_proxy_env_the_workload_gets_and_runs_under_dash_e() {
        let mut config = config_with_scripts(&["install psql"]);
        config.core.is_root = true;
        let scripts = crate::scripts::prepare(&config, None, &HashMap::new(), &NoPasswd)
            .expect("a script naming no user needs no lookup");
        assert_eq!(
            scripts[0].spec.env.get("HTTPS_PROXY").map(String::as_str),
            Some(format!("http://{}", config.core.proxy_listen_addr).as_str()),
            "a script that reached a mirror without the proxy would fail against the cage with a TLS error nobody could debug"
        );
        assert_eq!(
            scripts[0].spec.argv,
            ["sh", "-e", "/.lens/scripts/000.sh"],
            "§3.1.13 runs the body under -e, so the first failing command ends the script instead of a later one succeeding over it"
        );
    }

    #[tokio::test]
    async fn a_script_naming_a_group_the_guest_lacks_refuses_before_any_step() {
        let mut config = config_with_scripts(&["install psql"]);
        config.scripts[0].user = Some("1500:staff".into());
        let steps = Arc::new(RecordingSteps::default());
        let failure = run_scripts_before(
            &config,
            None,
            &HashMap::new(),
            &NoPasswd,
            steps.as_ref(),
            &ActivityStream::new(),
        )
        .await
        .expect_err("a group this guest cannot resolve refuses the run");
        assert!(
            steps.ran.lock().expect("uncontended").is_empty(),
            "a group is resolved with the rest of the identity before the first script spawns, so the run fails before it has half-prepared itself"
        );
        assert!(
            failure.to_string().contains("staff"),
            "the author needs to know which half of USER:GROUP this guest could not answer for; got: {failure}"
        );
    }

    #[tokio::test]
    async fn a_numeric_script_user_resolves_without_any_passwd_line() {
        let mut config = config_with_scripts(&["install psql", "seed the cache"]);
        config.scripts[0].user = Some("1500".into());
        config.scripts[1].user = Some("1500:77".into());
        let scripts = crate::scripts::prepare(&config, None, &HashMap::new(), &NoPasswd)
            .expect("a numeric identity needs no passwd line");
        let ids: Vec<(u32, u32)> = scripts
            .iter()
            .map(|s| {
                let creds = s.spec.creds.as_ref().expect("a non-root script setuids");
                let (uid, gid) = creds.uid_gid();
                (uid.as_raw(), gid.as_raw())
            })
            .collect();
        assert_eq!(
            ids,
            [(1500, 1500), (1500, 77)],
            "an image with a numeric USER and no matching passwd line must still be able to run a script, and a declared group still outranks the fallback"
        );
    }
}
