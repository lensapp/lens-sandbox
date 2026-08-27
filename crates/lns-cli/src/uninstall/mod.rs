use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use lns_ipc::{Request, Response, RunStatus, short_run_id};

use crate::command::{CommandSpec, subcommand};
use crate::connector::LocalBoxFuture;
use crate::service::{DisableOutcome, ServiceClient, TermInfo};

mod real;

const STOP_SERVICE_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_RUN_TIMEOUT_SECS: u64 = 10;

#[derive(clap::Args)]
pub struct UninstallArgs {
    #[arg(
        long,
        default_value_t = false,
        help = "Also delete everything under ~/.lns: cached artifacts and layers, named volumes, the audit trail, config, connectors, and stored credentials. Without this, only the program is removed and the directory is kept."
    )]
    pub purge: bool,

    #[arg(
        short = 'y',
        long = "yes",
        default_value_t = false,
        help = "Skip the confirmation prompt."
    )]
    pub yes: bool,
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(subcommand::<UninstallArgs>("uninstall").about(
        "Uninstall lns: stop running sandboxes, stop the background service, remove login auto-start, and delete the installed binaries.",
    ))
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "uninstall",
    augment,
    run: real::run_command,
    announces_update_check: false,
    owns_terminal: crate::command::never_owns_terminal,
};

/// Sends one request to the running service; `None` means the service did not answer.
pub trait UninstallService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>>;
}

pub trait LoginAgent {
    fn disable(&self) -> LocalBoxFuture<'_, DisableOutcome>;
}

pub trait Fs {
    fn remove_file(&self, path: &Path) -> LocalBoxFuture<'_, std::io::Result<()>>;
    fn remove_dir_all(&self, path: &Path) -> LocalBoxFuture<'_, std::io::Result<()>>;
}

#[derive(Default)]
pub struct UninstallPlan {
    pub binaries: Vec<PathBuf>,
    pub purge_dirs: Vec<PathBuf>,
    pub purge_files: Vec<PathBuf>,
}

pub struct Deps<'a, S, C, A, F> {
    pub svc: &'a S,
    pub client: &'a C,
    pub agent: &'a A,
    pub fs: &'a F,
}

pub async fn run_with<S, C, A, F>(
    args: &UninstallArgs,
    plan: &UninstallPlan,
    deps: &Deps<'_, S, C, A, F>,
    term: TermInfo,
    input: &mut dyn BufRead,
    writer: &mut impl Write,
    err: &mut (impl tokio::io::AsyncWriteExt + Unpin),
) -> Result<i32>
where
    S: UninstallService,
    C: ServiceClient,
    A: LoginAgent,
    F: Fs,
{
    if !args.yes {
        if !term.stdin_is_tty {
            bail!(
                "this stops your sandboxes and removes the lns binaries; there is no terminal to ask at, so pass -y/--yes to confirm"
            );
        }
        if !confirm(args.purge, &plan.purge_dirs, input, err).await? {
            err.write_all(b"Uninstall cancelled.\n").await?;
            err.flush().await?;
            return Ok(0);
        }
    }
    if deps.client.ping().await {
        stop_running_sandboxes(deps.svc, writer).await?;
    }
    report_disable(deps.agent.disable().await, writer)?;
    stop_service(deps.client, writer).await?;
    if args.purge {
        purge(deps.fs, plan, writer).await?;
    }
    remove_binaries(deps.fs, plan, writer).await?;
    writeln!(writer, "lns has been uninstalled.")?;
    Ok(0)
}

async fn confirm(
    purge: bool,
    purge_dirs: &[PathBuf],
    input: &mut dyn BufRead,
    err: &mut (impl tokio::io::AsyncWriteExt + Unpin),
) -> Result<bool> {
    err.write_all(question(purge, purge_dirs).as_bytes())
        .await?;
    err.flush().await?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    let answer = line.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

fn question(purge: bool, purge_dirs: &[PathBuf]) -> String {
    if !purge {
        return "This stops all running sandboxes and removes the lns binaries and background service. Your local data and named volumes are kept — re-run with --purge to remove them too. Continue? [y/N] ".to_string();
    }
    let targets = purge_dirs
        .iter()
        .map(|dir| dir.display().to_string())
        .collect::<Vec<_>>()
        .join(" and ");
    let data_clause = if targets.is_empty() {
        "deletes all local data".to_string()
    } else {
        format!("deletes all local data under {targets}")
    };
    format!(
        "This stops all running sandboxes, removes the lns binaries and background service, and {data_clause} (cached images, named volumes, the audit trail, and stored credentials). Continue? [y/N] "
    )
}

async fn stop_running_sandboxes(
    svc: &impl UninstallService,
    writer: &mut impl Write,
) -> Result<()> {
    let runs = match request_or_bail(svc, Request::ListRuns).await? {
        Response::RunList { runs } => runs,
        other => bail!("unexpected response from lns-service: {other:?}"),
    };
    for run in runs
        .into_iter()
        .filter(|r| matches!(r.status, RunStatus::Running))
    {
        stop_one(svc, &run.id, writer).await?;
    }
    Ok(())
}

async fn stop_one(svc: &impl UninstallService, id: &str, writer: &mut impl Write) -> Result<()> {
    let response = svc
        .request(Request::StopRun {
            run: id.to_string(),
            timeout_secs: STOP_RUN_TIMEOUT_SECS,
        })
        .await
        .ok_or_else(|| anyhow::anyhow!("no response from lns-service (is it running?)"))?;
    match response {
        Response::RunStopped { .. } => {
            writeln!(writer, "stopped sandbox {}", short_run_id(id))?;
            Ok(())
        }
        Response::Error { message } if crate::sandbox::is_unknown_run(&message) => Ok(()),
        Response::Error { message } => bail!("{message}"),
        other => bail!("unexpected response from lns-service: {other:?}"),
    }
}

async fn request_or_bail(svc: &impl UninstallService, req: Request) -> Result<Response> {
    let response = svc
        .request(req)
        .await
        .ok_or_else(|| anyhow::anyhow!("no response from lns-service (is it running?)"))?;
    if let Response::Error { message } = &response {
        bail!("{message}");
    }
    Ok(response)
}

fn report_disable(outcome: DisableOutcome, writer: &mut impl Write) -> Result<()> {
    match outcome {
        DisableOutcome::Unregistered => writeln!(writer, "removed login auto-start")?,
        DisableOutcome::WasNotRegistered => {}
    }
    Ok(())
}

async fn stop_service(client: &impl ServiceClient, writer: &mut impl Write) -> Result<()> {
    let acknowledged = client.shutdown().await.is_some();
    if !client.wait_for_stopped(STOP_SERVICE_TIMEOUT).await {
        bail!(
            "the background service did not stop within {}s — nothing was removed; retry once it has stopped",
            STOP_SERVICE_TIMEOUT.as_secs()
        );
    }
    if acknowledged {
        writeln!(writer, "stopped the background service")?;
    }
    Ok(())
}

async fn purge(fs: &impl Fs, plan: &UninstallPlan, writer: &mut impl Write) -> Result<()> {
    for dir in &plan.purge_dirs {
        removed(fs.remove_dir_all(dir).await, dir, writer)?;
    }
    for file in &plan.purge_files {
        removed(fs.remove_file(file).await, file, writer)?;
    }
    Ok(())
}

fn removed(result: std::io::Result<()>, path: &Path, writer: &mut impl Write) -> Result<()> {
    match result {
        Ok(()) => {
            writeln!(writer, "removed {}", path.display())?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            Err(anyhow::Error::new(e)).with_context(|| format!("removing {}", path.display()))
        }
    }
}

async fn remove_binaries(
    fs: &impl Fs,
    plan: &UninstallPlan,
    writer: &mut impl Write,
) -> Result<()> {
    for bin in &plan.binaries {
        match fs.remove_file(bin).await {
            Ok(()) => writeln!(writer, "removed {}", bin.display())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(anyhow::Error::new(e)).with_context(|| {
                    format!(
                        "removing {} (insufficient permissions? re-run with sudo)",
                        bin.display()
                    )
                });
            }
        }
    }
    Ok(())
}

/// What `--purge` clears: the one directory lns keeps everything in, and the socket, which lives with the service rather than with your data.
pub(crate) struct PurgeSources {
    pub lns_home: PathBuf,
    pub home: Option<PathBuf>,
    pub socket: PathBuf,
    pub socket_overridden: bool,
}

pub(crate) fn purge_targets(src: PurgeSources) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    if !src.lns_home.is_absolute()
        || src.lns_home.parent().is_none()
        || src.home.as_deref() == Some(src.lns_home.as_path())
    {
        bail!(
            "refusing to purge {}: the lns data root must be an absolute directory of its own, not the filesystem root or your home directory — check LNS_HOME",
            src.lns_home.display()
        );
    }
    let mut dirs = vec![src.lns_home];
    let mut files = Vec::new();
    // A socket at its default lns-owned location takes its whole parent directory; an env-overridden one may live anywhere, so only the socket and the log beside it go.
    match (src.socket_overridden, src.socket.parent()) {
        (false, Some(parent)) => dirs.push(parent.to_path_buf()),
        _ => {
            if let Some(log) = src.socket.parent().map(|p| p.join("service.log")) {
                files.push(log);
            }
            files.push(src.socket);
        }
    }
    Ok((dirs, files))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeService {
        list: Option<Option<Response>>,
        stop: Mutex<HashMap<String, Option<Response>>>,
        requests: Mutex<Vec<Request>>,
    }

    impl FakeService {
        fn with_runs(runs: Vec<lns_ipc::RunSummary>) -> Self {
            Self {
                list: Some(Some(Response::RunList { runs })),
                ..Self::default()
            }
        }

        fn stop_reply(self, id: &str, response: Option<Response>) -> Self {
            self.stop.lock().unwrap().insert(id.to_string(), response);
            self
        }

        fn requests(&self) -> Vec<Request> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl UninstallService for FakeService {
        fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>> {
            self.requests.lock().unwrap().push(req.clone());
            let reply = match &req {
                Request::ListRuns => self
                    .list
                    .clone()
                    .expect("FakeService: ListRuns with no queued reply"),
                Request::StopRun { run, .. } => self
                    .stop
                    .lock()
                    .unwrap()
                    .get(run)
                    .cloned()
                    .unwrap_or(Some(Response::RunStopped { forced: false })),
                other => panic!("FakeService: unexpected request {other:?}"),
            };
            Box::pin(async move { reply })
        }
    }

    #[derive(Default)]
    struct FakeClient {
        ping: bool,
        shutdown: Option<()>,
        wait_for_stopped: bool,
        calls: Mutex<Vec<&'static str>>,
    }

    impl FakeClient {
        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ServiceClient for FakeClient {
        fn ping(&self) -> crate::service::client::BoxFuture<'_, bool> {
            self.calls.lock().unwrap().push("ping");
            let v = self.ping;
            Box::pin(async move { v })
        }
        fn shutdown(&self) -> crate::service::client::BoxFuture<'_, Option<()>> {
            self.calls.lock().unwrap().push("shutdown");
            let v = self.shutdown;
            Box::pin(async move { v })
        }
        fn wait_for_stopped(&self, _t: Duration) -> crate::service::client::BoxFuture<'_, bool> {
            self.calls.lock().unwrap().push("wait_for_stopped");
            let v = self.wait_for_stopped;
            Box::pin(async move { v })
        }
        fn status(&self) -> crate::service::client::BoxFuture<'_, Option<lns_ipc::StatusInfo>> {
            unreachable!("uninstall never calls status()")
        }
        fn start_and_wait_for_ready(
            &self,
            _t: Duration,
        ) -> crate::service::client::BoxFuture<'_, Result<bool>> {
            unreachable!("uninstall never calls start_and_wait_for_ready()")
        }
        fn wait_for_ready(&self, _t: Duration) -> crate::service::client::BoxFuture<'_, bool> {
            unreachable!("uninstall never calls wait_for_ready()")
        }
        fn cancel_run(&self, _run_id: String) -> crate::service::client::BoxFuture<'_, ()> {
            unreachable!("uninstall never calls cancel_run()")
        }
    }

    struct FakeAgent {
        outcome: DisableOutcome,
        calls: Mutex<u32>,
    }

    impl FakeAgent {
        fn new(outcome: DisableOutcome) -> Self {
            Self {
                outcome,
                calls: Mutex::new(0),
            }
        }
    }

    impl LoginAgent for FakeAgent {
        fn disable(&self) -> LocalBoxFuture<'_, DisableOutcome> {
            *self.calls.lock().unwrap() += 1;
            let outcome = self.outcome;
            Box::pin(async move { outcome })
        }
    }

    #[derive(Default)]
    struct FakeFs {
        errors: HashMap<PathBuf, std::io::ErrorKind>,
        removed: Mutex<Vec<PathBuf>>,
    }

    impl FakeFs {
        fn failing(path: &Path, kind: std::io::ErrorKind) -> Self {
            let mut errors = HashMap::new();
            errors.insert(path.to_path_buf(), kind);
            Self {
                errors,
                ..Self::default()
            }
        }

        fn removed(&self) -> Vec<PathBuf> {
            self.removed.lock().unwrap().clone()
        }

        fn outcome(&self, path: &Path) -> std::io::Result<()> {
            if let Some(kind) = self.errors.get(path) {
                return Err(std::io::Error::from(*kind));
            }
            self.removed.lock().unwrap().push(path.to_path_buf());
            Ok(())
        }
    }

    impl Fs for FakeFs {
        fn remove_file(&self, path: &Path) -> LocalBoxFuture<'_, std::io::Result<()>> {
            let r = self.outcome(path);
            Box::pin(async move { r })
        }
        fn remove_dir_all(&self, path: &Path) -> LocalBoxFuture<'_, std::io::Result<()>> {
            let r = self.outcome(path);
            Box::pin(async move { r })
        }
    }

    fn running(id: &str) -> lns_ipc::RunSummary {
        lns_ipc::RunSummary {
            id: id.to_string(),
            name: "agent".into(),
            image: "some-image".into(),
            command: "cmd".into(),
            status: RunStatus::Running,
            started: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn exited(id: &str) -> lns_ipc::RunSummary {
        lns_ipc::RunSummary {
            status: RunStatus::Exited { code: 0 },
            ..running(id)
        }
    }

    fn args(purge: bool, yes: bool) -> UninstallArgs {
        UninstallArgs { purge, yes }
    }

    fn plan_with_binaries(binaries: &[&str]) -> UninstallPlan {
        UninstallPlan {
            binaries: binaries.iter().map(PathBuf::from).collect(),
            ..UninstallPlan::default()
        }
    }

    struct Rig {
        svc: FakeService,
        client: FakeClient,
        agent: FakeAgent,
        fs: FakeFs,
    }

    fn at_a_terminal() -> TermInfo {
        TermInfo {
            stdin_is_tty: true,
            stdout_is_terminal: true,
        }
    }

    impl Rig {
        async fn run(
            &self,
            args: &UninstallArgs,
            plan: &UninstallPlan,
            input: &str,
        ) -> (Result<i32>, String) {
            let (result, out, _err) = self.run_at(args, plan, at_a_terminal(), input).await;
            (result, out)
        }

        async fn run_at(
            &self,
            args: &UninstallArgs,
            plan: &UninstallPlan,
            term: TermInfo,
            input: &str,
        ) -> (Result<i32>, String, String) {
            let mut reader = input.as_bytes();
            let mut out: Vec<u8> = Vec::new();
            let mut err: Vec<u8> = Vec::new();
            let deps = Deps {
                svc: &self.svc,
                client: &self.client,
                agent: &self.agent,
                fs: &self.fs,
            };
            let result = run_with(args, plan, &deps, term, &mut reader, &mut out, &mut err).await;
            (
                result,
                String::from_utf8(out).unwrap(),
                String::from_utf8(err).unwrap(),
            )
        }
    }

    fn rig(svc: FakeService, client: FakeClient, agent: FakeAgent, fs: FakeFs) -> Rig {
        Rig {
            svc,
            client,
            agent,
            fs,
        }
    }

    fn stopped_client() -> FakeClient {
        FakeClient {
            wait_for_stopped: true,
            ..FakeClient::default()
        }
    }

    fn running_client() -> FakeClient {
        FakeClient {
            ping: true,
            shutdown: Some(()),
            wait_for_stopped: true,
            ..FakeClient::default()
        }
    }

    #[tokio::test]
    async fn declining_the_prompt_cancels_before_touching_anything() {
        let rig = rig(
            FakeService::default(),
            stopped_client(),
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs::default(),
        );
        let (code, out, err) = rig
            .run_at(
                &args(false, false),
                &plan_with_binaries(&["/bin/lns"]),
                at_a_terminal(),
                "n\n",
            )
            .await;
        assert_eq!(code.unwrap(), 0);
        assert!(err.contains("Uninstall cancelled."), "got: {err}");
        assert!(!out.contains("Uninstall cancelled."), "got: {out}");
        assert!(rig.client.calls().is_empty(), "service must not be pinged");
        assert!(rig.fs.removed().is_empty(), "nothing removed");
        assert_eq!(*rig.agent.calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn with_no_terminal_to_ask_at_the_uninstall_refuses_and_names_the_flag_that_answers_it() {
        let rig = rig(
            FakeService::default(),
            stopped_client(),
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs::default(),
        );
        let (result, _out, _err) = rig
            .run_at(
                &args(false, false),
                &plan_with_binaries(&["/bin/lns"]),
                TermInfo::default(),
                "",
            )
            .await;
        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains("--yes"), "got: {err}");
        assert!(err.contains("no terminal to ask at"), "got: {err}");
        assert!(rig.fs.removed().is_empty(), "nothing removed");
        assert!(rig.client.calls().is_empty(), "service must not be pinged");
    }

    #[tokio::test]
    async fn the_confirmation_question_is_asked_on_stderr_so_a_piped_stdout_never_hides_it() {
        let rig = rig(
            FakeService::default(),
            stopped_client(),
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs::default(),
        );
        let (code, out, err) = rig
            .run_at(
                &args(false, false),
                &plan_with_binaries(&["/bin/lns"]),
                at_a_terminal(),
                "n\n",
            )
            .await;
        assert_eq!(code.unwrap(), 0);
        assert!(err.contains("Continue? [y/N]"), "got: {err}");
        assert!(!out.contains("Continue?"), "got: {out}");
    }

    #[tokio::test]
    async fn empty_answer_is_treated_as_no() {
        let rig = rig(
            FakeService::default(),
            stopped_client(),
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs::default(),
        );
        let (code, _out, err) = rig
            .run_at(
                &args(false, false),
                &plan_with_binaries(&["/bin/lns"]),
                at_a_terminal(),
                "\n",
            )
            .await;
        assert_eq!(code.unwrap(), 0);
        assert!(err.contains("Uninstall cancelled."), "got: {err}");
    }

    #[tokio::test]
    async fn accepting_the_prompt_runs_the_full_removal() {
        let rig = rig(
            FakeService::default(),
            stopped_client(),
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs::default(),
        );
        let (code, out) = rig
            .run(
                &args(false, false),
                &plan_with_binaries(&["/bin/lns", "/bin/lns-service"]),
                "y\n",
            )
            .await;
        assert_eq!(code.unwrap(), 0);
        assert!(out.contains("lns has been uninstalled."), "got: {out}");
        assert_eq!(
            rig.fs.removed(),
            vec![PathBuf::from("/bin/lns"), PathBuf::from("/bin/lns-service")]
        );
    }

    #[tokio::test]
    async fn yes_flag_skips_the_prompt() {
        let rig = rig(
            FakeService::default(),
            stopped_client(),
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs::default(),
        );
        let (code, out, err) = rig
            .run_at(
                &args(false, true),
                &plan_with_binaries(&["/bin/lns"]),
                at_a_terminal(),
                "",
            )
            .await;
        assert_eq!(code.unwrap(), 0);
        assert!(!err.contains("Continue?"), "prompt must be skipped: {err}");
        assert!(out.contains("lns has been uninstalled."), "got: {out}");
    }

    #[tokio::test]
    async fn running_service_stops_only_running_sandboxes_before_removal() {
        let svc = FakeService::with_runs(vec![running("aa01"), exited("bb02"), running("cc03")]);
        let rig = rig(
            svc,
            running_client(),
            FakeAgent::new(DisableOutcome::Unregistered),
            FakeFs::default(),
        );
        let (code, out) = rig
            .run(&args(false, true), &plan_with_binaries(&["/bin/lns"]), "")
            .await;
        assert_eq!(code.unwrap(), 0);
        let requests = rig.svc.requests();
        assert!(matches!(requests[0], Request::ListRuns));
        let stopped: Vec<&str> = requests
            .iter()
            .filter_map(|r| match r {
                Request::StopRun { run, .. } => Some(run.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            stopped,
            vec!["aa01", "cc03"],
            "only running runs are stopped"
        );
        assert_eq!(
            rig.client.calls(),
            vec!["ping", "shutdown", "wait_for_stopped"]
        );
        assert!(out.contains("removed login auto-start"), "got: {out}");
        assert!(out.contains("stopped the background service"), "got: {out}");
    }

    #[tokio::test]
    async fn stopped_service_skips_run_stopping_but_still_unregisters_and_removes() {
        let rig = rig(
            FakeService::default(),
            stopped_client(),
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs::default(),
        );
        let (code, out) = rig
            .run(&args(false, true), &plan_with_binaries(&["/bin/lns"]), "")
            .await;
        assert_eq!(code.unwrap(), 0);
        assert!(rig.svc.requests().is_empty(), "no run requests when down");
        assert_eq!(
            rig.client.calls(),
            vec!["ping", "shutdown", "wait_for_stopped"]
        );
        assert!(!out.contains("removed login auto-start"), "got: {out}");
        assert_eq!(rig.fs.removed(), vec![PathBuf::from("/bin/lns")]);
    }

    #[tokio::test]
    async fn a_run_that_raced_to_exit_is_treated_as_already_stopped() {
        let svc = FakeService::with_runs(vec![running("aa01")]).stop_reply(
            "aa01",
            Some(Response::Error {
                message: "no active run with id aa01".into(),
            }),
        );
        let rig = rig(
            svc,
            running_client(),
            FakeAgent::new(DisableOutcome::Unregistered),
            FakeFs::default(),
        );
        let (code, out) = rig
            .run(&args(false, true), &plan_with_binaries(&["/bin/lns"]), "")
            .await;
        assert_eq!(code.unwrap(), 0);
        assert!(
            !out.contains("stopped sandbox"),
            "raced run is silent: {out}"
        );
        assert!(out.contains("lns has been uninstalled."));
    }

    #[tokio::test]
    async fn a_forced_stop_still_reports_the_sandbox_stopped() {
        let svc = FakeService::with_runs(vec![running("aa01")])
            .stop_reply("aa01", Some(Response::RunStopped { forced: true }));
        let rig = rig(
            svc,
            running_client(),
            FakeAgent::new(DisableOutcome::Unregistered),
            FakeFs::default(),
        );
        let (code, out) = rig
            .run(&args(false, true), &plan_with_binaries(&["/bin/lns"]), "")
            .await;
        assert_eq!(code.unwrap(), 0);
        assert!(out.contains("stopped sandbox aa01"), "got: {out}");
    }

    #[tokio::test]
    async fn a_real_stop_error_aborts_before_any_removal() {
        let svc = FakeService::with_runs(vec![running("aa01")]).stop_reply(
            "aa01",
            Some(Response::Error {
                message: "guest is wedged".into(),
            }),
        );
        let rig = rig(
            svc,
            running_client(),
            FakeAgent::new(DisableOutcome::Unregistered),
            FakeFs::default(),
        );
        let (result, _out) = rig
            .run(&args(false, true), &plan_with_binaries(&["/bin/lns"]), "")
            .await;
        let err = result.unwrap_err();
        assert!(format!("{err:#}").contains("guest is wedged"), "{err:#}");
        assert!(rig.fs.removed().is_empty(), "binaries untouched on failure");
    }

    #[tokio::test]
    async fn a_stop_with_no_response_aborts() {
        let svc = FakeService::with_runs(vec![running("aa01")]).stop_reply("aa01", None);
        let rig = rig(
            svc,
            running_client(),
            FakeAgent::new(DisableOutcome::Unregistered),
            FakeFs::default(),
        );
        let (result, _out) = rig
            .run(&args(false, true), &plan_with_binaries(&["/bin/lns"]), "")
            .await;
        let err = result.unwrap_err();
        assert!(format!("{err:#}").contains("no response"), "{err:#}");
    }

    #[tokio::test]
    async fn a_stop_with_an_unexpected_response_aborts() {
        let svc =
            FakeService::with_runs(vec![running("aa01")]).stop_reply("aa01", Some(Response::Pong));
        let rig = rig(
            svc,
            running_client(),
            FakeAgent::new(DisableOutcome::Unregistered),
            FakeFs::default(),
        );
        let (result, _out) = rig
            .run(&args(false, true), &plan_with_binaries(&["/bin/lns"]), "")
            .await;
        assert!(
            format!("{:#}", result.unwrap_err()).contains("unexpected response"),
            "expected unexpected-response error"
        );
    }

    #[tokio::test]
    async fn a_listing_error_aborts() {
        let svc = FakeService {
            list: Some(Some(Response::Error {
                message: "registry poisoned".into(),
            })),
            ..FakeService::default()
        };
        let rig = rig(
            svc,
            running_client(),
            FakeAgent::new(DisableOutcome::Unregistered),
            FakeFs::default(),
        );
        let (result, _out) = rig
            .run(&args(false, true), &plan_with_binaries(&["/bin/lns"]), "")
            .await;
        assert!(format!("{:#}", result.unwrap_err()).contains("registry poisoned"));
    }

    #[tokio::test]
    async fn a_listing_with_no_response_aborts() {
        let svc = FakeService {
            list: Some(None),
            ..FakeService::default()
        };
        let rig = rig(
            svc,
            running_client(),
            FakeAgent::new(DisableOutcome::Unregistered),
            FakeFs::default(),
        );
        let (result, _out) = rig
            .run(&args(false, true), &plan_with_binaries(&["/bin/lns"]), "")
            .await;
        assert!(format!("{:#}", result.unwrap_err()).contains("no response"));
    }

    #[tokio::test]
    async fn a_listing_with_an_unexpected_response_aborts() {
        let svc = FakeService {
            list: Some(Some(Response::Pong)),
            ..FakeService::default()
        };
        let rig = rig(
            svc,
            running_client(),
            FakeAgent::new(DisableOutcome::Unregistered),
            FakeFs::default(),
        );
        let (result, _out) = rig
            .run(&args(false, true), &plan_with_binaries(&["/bin/lns"]), "")
            .await;
        assert!(format!("{:#}", result.unwrap_err()).contains("unexpected response"));
    }

    #[tokio::test]
    async fn a_shutdown_with_no_reply_still_waits_before_removing_anything() {
        let client = FakeClient {
            ping: true,
            shutdown: None,
            wait_for_stopped: false,
            ..FakeClient::default()
        };
        let rig = rig(
            FakeService::with_runs(vec![]),
            client,
            FakeAgent::new(DisableOutcome::Unregistered),
            FakeFs::default(),
        );
        let (result, _out) = rig
            .run(&args(false, true), &plan_with_binaries(&["/bin/lns"]), "")
            .await;
        let err = result.unwrap_err();
        assert!(format!("{err:#}").contains("did not stop"), "{err:#}");
        assert!(
            rig.fs.removed().is_empty(),
            "must not remove anything until the service is confirmed down"
        );
        assert_eq!(
            rig.client.calls(),
            vec!["ping", "shutdown", "wait_for_stopped"],
            "a no-reply shutdown must still be followed by the stopped-wait"
        );
    }

    #[tokio::test]
    async fn a_shutdown_that_never_exits_aborts_before_removal() {
        let client = FakeClient {
            ping: false,
            shutdown: Some(()),
            wait_for_stopped: false,
            ..FakeClient::default()
        };
        let rig = rig(
            FakeService::default(),
            client,
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs::default(),
        );
        let (result, _out) = rig
            .run(&args(false, true), &plan_with_binaries(&["/bin/lns"]), "")
            .await;
        let err = result.unwrap_err();
        assert!(format!("{err:#}").contains("did not stop"), "{err:#}");
        assert!(
            rig.fs.removed().is_empty(),
            "nothing removed on stop timeout"
        );
    }

    #[tokio::test]
    async fn purge_removes_the_one_directory_and_the_socket_then_the_binaries() {
        let plan = UninstallPlan {
            binaries: vec![PathBuf::from("/bin/lns")],
            purge_dirs: vec![PathBuf::from("/home/me/.lns")],
            purge_files: vec![PathBuf::from("/run/user/1000/lns/service.log")],
        };
        let rig = rig(
            FakeService::default(),
            stopped_client(),
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs::default(),
        );
        let (code, out) = rig.run(&args(true, true), &plan, "").await;
        assert_eq!(code.unwrap(), 0);
        assert_eq!(
            rig.fs.removed(),
            vec![
                PathBuf::from("/home/me/.lns"),
                PathBuf::from("/run/user/1000/lns/service.log"),
                PathBuf::from("/bin/lns"),
            ]
        );
        assert!(out.contains("removed /home/me/.lns"), "got: {out}");
    }

    #[tokio::test]
    async fn purge_confirmation_names_data_deletion() {
        let rig = rig(
            FakeService::default(),
            stopped_client(),
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs::default(),
        );
        let (code, _out, err) = rig
            .run_at(
                &args(true, false),
                &plan_with_binaries(&["/bin/lns"]),
                at_a_terminal(),
                "n\n",
            )
            .await;
        assert_eq!(code.unwrap(), 0);
        assert!(
            err.contains("deletes all local data"),
            "purge wording: {err}"
        );
    }

    #[tokio::test]
    async fn purge_confirmation_shows_the_resolved_roots_it_will_delete() {
        let rig = rig(
            FakeService::default(),
            stopped_client(),
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs::default(),
        );
        let plan = UninstallPlan {
            binaries: vec![PathBuf::from("/bin/lns")],
            purge_dirs: vec![
                PathBuf::from("/home/me/.lns"),
                PathBuf::from("/run/user/1000/lns"),
            ],
            purge_files: Vec::new(),
        };
        let (code, _out, err) = rig
            .run_at(&args(true, false), &plan, at_a_terminal(), "n\n")
            .await;
        assert_eq!(code.unwrap(), 0);
        assert!(
            err.contains("/home/me/.lns") && err.contains("/run/user/1000/lns"),
            "the prompt must show what will actually be deleted: {err}"
        );
    }

    #[tokio::test]
    async fn non_purge_confirmation_says_data_is_kept() {
        let rig = rig(
            FakeService::default(),
            stopped_client(),
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs::default(),
        );
        let (_code, _out, err) = rig
            .run_at(
                &args(false, false),
                &plan_with_binaries(&["/bin/lns"]),
                at_a_terminal(),
                "n\n",
            )
            .await;
        assert!(err.contains("kept"), "non-purge keeps data: {err}");
    }

    #[tokio::test]
    async fn purge_surfaces_a_data_removal_error_and_skips_binaries() {
        let plan = UninstallPlan {
            binaries: vec![PathBuf::from("/bin/lns")],
            purge_dirs: vec![PathBuf::from("/cache/lns")],
            ..UninstallPlan::default()
        };
        let rig = rig(
            FakeService::default(),
            stopped_client(),
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs::failing(
                Path::new("/cache/lns"),
                std::io::ErrorKind::PermissionDenied,
            ),
        );
        let (result, _out) = rig.run(&args(true, true), &plan, "").await;
        let err = result.unwrap_err();
        assert!(
            format!("{err:#}").contains("removing /cache/lns"),
            "{err:#}"
        );
        assert!(rig.fs.removed().is_empty(), "binaries not reached");
    }

    #[tokio::test]
    async fn purge_tolerates_already_removed_data() {
        let plan = UninstallPlan {
            binaries: vec![PathBuf::from("/bin/lns")],
            purge_dirs: vec![PathBuf::from("/home/me/.lns")],
            purge_files: vec![PathBuf::from("/run/user/1000/lns/service.log")],
        };
        let rig = rig(
            FakeService::default(),
            stopped_client(),
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs {
                errors: [
                    (PathBuf::from("/home/me/.lns"), std::io::ErrorKind::NotFound),
                    (
                        PathBuf::from("/run/user/1000/lns/service.log"),
                        std::io::ErrorKind::NotFound,
                    ),
                ]
                .into_iter()
                .collect(),
                ..FakeFs::default()
            },
        );
        let (code, _out) = rig.run(&args(true, true), &plan, "").await;
        assert_eq!(code.unwrap(), 0);
        assert_eq!(rig.fs.removed(), vec![PathBuf::from("/bin/lns")]);
    }

    #[tokio::test]
    async fn a_missing_binary_is_tolerated_on_rerun() {
        let rig = rig(
            FakeService::default(),
            stopped_client(),
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs::failing(Path::new("/bin/lns"), std::io::ErrorKind::NotFound),
        );
        let (code, out) = rig
            .run(
                &args(false, true),
                &plan_with_binaries(&["/bin/lns", "/bin/lns-service"]),
                "",
            )
            .await;
        assert_eq!(code.unwrap(), 0);
        assert_eq!(rig.fs.removed(), vec![PathBuf::from("/bin/lns-service")]);
        assert!(out.contains("lns has been uninstalled."));
    }

    #[tokio::test]
    async fn a_permission_denied_binary_removal_is_actionable() {
        let rig = rig(
            FakeService::default(),
            stopped_client(),
            FakeAgent::new(DisableOutcome::WasNotRegistered),
            FakeFs::failing(
                Path::new("/usr/local/bin/lns"),
                std::io::ErrorKind::PermissionDenied,
            ),
        );
        let (result, _out) = rig
            .run(
                &args(false, true),
                &plan_with_binaries(&["/usr/local/bin/lns"]),
                "",
            )
            .await;
        let err = result.unwrap_err();
        assert!(format!("{err:#}").contains("re-run with sudo"), "{err:#}");
    }

    fn sources() -> PurgeSources {
        PurgeSources {
            lns_home: PathBuf::from("/home/me/.lns"),
            home: Some(PathBuf::from("/home/me")),
            socket: PathBuf::from("/run/user/1000/lns/service.sock"),
            socket_overridden: false,
        }
    }

    #[test]
    fn purge_refuses_the_filesystem_root_as_a_data_root() {
        let err = purge_targets(PurgeSources {
            lns_home: PathBuf::from("/"),
            ..sources()
        })
        .unwrap_err();
        assert!(
            err.to_string().contains('/') && err.to_string().contains("refusing"),
            "an LNS_HOME of / must never become an rm -rf target: {err}"
        );
    }

    #[test]
    fn purge_refuses_your_home_directory_itself_as_a_data_root() {
        let err = purge_targets(PurgeSources {
            lns_home: PathBuf::from("/home/me"),
            ..sources()
        })
        .unwrap_err();
        assert!(
            err.to_string().contains("/home/me"),
            "LNS_HOME=$HOME must be refused, not deleted: {err}"
        );
    }

    #[test]
    fn purge_refuses_a_relative_data_root() {
        for bad in [".", "relative/dir"] {
            let err = purge_targets(PurgeSources {
                lns_home: PathBuf::from(bad),
                ..sources()
            })
            .unwrap_err();
            assert!(
                err.to_string().contains("refusing"),
                "{bad:?} must be refused: {err}"
            );
        }
    }

    #[test]
    fn purge_takes_the_one_directory_lns_keeps_everything_in() {
        let (dirs, files) = purge_targets(sources()).unwrap();
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/home/me/.lns"),
                PathBuf::from("/run/user/1000/lns"),
            ],
            "one root means one thing to delete, plus the socket's own directory"
        );
        assert!(files.is_empty(), "got: {files:?}");
    }

    #[test]
    fn an_overridden_socket_removes_the_socket_and_log_files_never_its_parent() {
        let (dirs, files) = purge_targets(PurgeSources {
            socket_overridden: true,
            ..sources()
        })
        .unwrap();
        assert!(
            !dirs.contains(&PathBuf::from("/run/user/1000/lns")),
            "an override's arbitrary parent must never be rm -rf'd"
        );
        assert!(files.contains(&PathBuf::from("/run/user/1000/lns/service.sock")));
        assert!(files.contains(&PathBuf::from("/run/user/1000/lns/service.log")));
    }

    #[test]
    fn a_parentless_socket_is_removed_as_a_file() {
        let (dirs, files) = purge_targets(PurgeSources {
            socket: PathBuf::from("/"),
            ..sources()
        })
        .unwrap();
        assert_eq!(dirs, vec![PathBuf::from("/home/me/.lns")]);
        assert_eq!(files, vec![PathBuf::from("/")]);
    }

    #[test]
    #[should_panic(expected = "unexpected request")]
    fn fake_service_rejects_an_unexpected_request() {
        drop(FakeService::default().request(Request::Ping));
    }

    #[test]
    #[should_panic(expected = "uninstall never calls status()")]
    fn fake_client_status_is_a_regression_tripwire() {
        drop(FakeClient::default().status());
    }

    #[test]
    #[should_panic(expected = "uninstall never calls start_and_wait_for_ready()")]
    fn fake_client_start_is_a_regression_tripwire() {
        drop(FakeClient::default().start_and_wait_for_ready(Duration::from_secs(0)));
    }

    #[test]
    #[should_panic(expected = "uninstall never calls wait_for_ready()")]
    fn fake_client_wait_for_ready_is_a_regression_tripwire() {
        drop(FakeClient::default().wait_for_ready(Duration::from_secs(0)));
    }

    #[test]
    #[should_panic(expected = "uninstall never calls cancel_run()")]
    fn fake_client_cancel_run_is_a_regression_tripwire() {
        drop(FakeClient::default().cancel_run(String::new()));
    }
}
