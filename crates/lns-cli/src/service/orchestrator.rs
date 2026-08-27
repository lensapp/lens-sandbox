use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use lns_ipc::{
    LogLevel, Request, Response, SignalKind, WireFrame, decode_frame, decode_wire_frame_from_bytes,
    encode_frame, read_frame_bytes_async,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

use clap::FromArgMatches;

use crate::chord::{DetachChordDetector, FeedAction};
use crate::cli::{ExecArgs, RunArgs};
use crate::command::{RunCtx, RunFuture};
use lns_ipc::{ExecImageArgs, RunImageArgs};

use super::{client::ServiceClient, real, require_running_check};
use crate::run::summary::print_run_summary;

const PUBLISHED_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(30);

fn primary_target(run_id: impl Into<String>) -> lns_ipc::SessionTarget {
    lns_ipc::SessionTarget::Primary {
        run_id: run_id.into(),
    }
}

/// §5: a `run` or `exec` that fails is `lns` failing, never the workload — the workload's own status comes back as a code, so an error here is always the 125 case and is never mistaken for a workload that exited 1.
pub const PRE_START_FAILURE: i32 = 125;

pub fn as_pre_start_failure(result: Result<i32>) -> i32 {
    match result {
        Ok(code) => code,
        Err(e) => {
            crate::log::error!("{e:#}");
            PRE_START_FAILURE
        }
    }
}

pub fn run_command<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let started = async {
            let args = RunArgs::from_arg_matches(matches)?;
            launch_run(args, ctx.debug).await
        };
        Ok(as_pre_start_failure(started.await))
    })
}

pub async fn launch_run(mut args: RunArgs, debug: bool) -> Result<i32> {
    args.env = crate::run::env_file::merged_run_env(&args.env_file, &args.env)?;
    let config_path = crate::config::default_config_path()?;
    let defaults = crate::config::load_run_defaults(&config_path)?;
    let args = crate::config::apply_run_defaults(args, defaults);
    let cwd = std::env::current_dir().context("reading current directory")?;
    // Target resolution is offline; a missing or invalid definition fails fast, before the service gate.
    let target = crate::run::target::resolve(
        args.image.as_deref(),
        args.file.as_deref(),
        &crate::artifact::real::RealFs,
        &cwd,
    )?;
    require_running().await?;
    run_image(args, target, cwd, debug).await
}

pub fn exec_command<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let started = async {
            let args = ExecArgs::from_arg_matches(matches)?;
            if let Some(refusal) = crate::sandbox::document_refusal("exec", &args.run) {
                eprintln!("error: {refusal}");
                return Ok(2);
            }
            require_running().await?;
            exec_image(args).await
        };
        Ok(as_pre_start_failure(started.await))
    })
}

pub fn service_command<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::ServiceArgs::from_arg_matches(matches)?;
        let mut out = ctx.out;
        dispatch(&args.command, &mut out).await?;
        Ok(0)
    })
}

pub async fn dispatch(cmd: &super::ServiceCommand, writer: &mut dyn std::io::Write) -> Result<()> {
    let client = real_client()?;
    match cmd {
        super::ServiceCommand::Start => super::cmd_start(&client).await,
        super::ServiceCommand::Stop => super::cmd_stop(&client).await,
        super::ServiceCommand::Status(args) => super::cmd_status(&client, args, writer).await,
        super::ServiceCommand::Enable => super::cmd_enable(&client).await,
        super::ServiceCommand::Disable => super::cmd_disable(&client).await,
    }
}

/// The caller decides what an unreachable service costs: `run` and `exec` answer 125 for it (§5), every other verb the ordinary 1.
pub async fn require_running() -> Result<()> {
    let client = real_client()?;
    let alive = client.ping().await;
    require_running_check(alive).map_err(anyhow::Error::msg)
}
fn real_client() -> Result<real::RealServiceClient> {
    Ok(real::RealServiceClient::new(
        super::socket_path()?,
        super::find_service_binary(),
    ))
}

/// A detached run must never block on the KEEP/DROP prompt, so it drops undecided secrets like a no-terminal run even when launched from a TTY.
fn host_binds_interactive(detached: bool, stdin_is_tty: bool) -> bool {
    !detached && stdin_is_tty
}

/// Takes the caller's input because the caller holds the process-wide stdin lock, which is not reentrant.
fn resolve_host_binds(
    specs: &[lns_ipc::BindSpec],
    interactive: bool,
    input: &mut dyn std::io::BufRead,
    store: &lns_policy::host_bind_decisions::HostBindDecisionStore,
) -> Result<Vec<crate::run::host_bind::ResolvedBind>> {
    if specs.is_empty() {
        return Ok(Vec::new());
    }
    let scan = crate::run::host_bind::RealDirScan;
    crate::run::host_bind::resolve_binds(
        specs,
        &scan,
        store,
        interactive,
        input,
        &mut std::io::stderr(),
    )
}

/// Every question a run asks before it boots, and the stores that remember the answers.
struct PreBootQuestions<'a> {
    pulled: Option<crate::run::pull_confirm::PulledEffects<'a>>,
    origin: crate::run::host_path_consent::DocumentOrigin,
    filesets: &'a [crate::run::summary::FilesetSummary],
    host_paths: &'a lns_policy::host_path_decisions::HostPathDecisionStore,
    bind_specs: &'a [lns_ipc::BindSpec],
    bind_decisions: &'a lns_policy::host_bind_decisions::HostBindDecisionStore,
    assume_yes: bool,
    interactive: bool,
}

/// Asks all of them under one stdin guard and drops it before returning: an attached session reads the tty through tokio's stdin, whose blocking thread takes this same lock, so a guard that outlives the questions leaves the session with no keyboard.
fn ask_before_boot(
    q: &PreBootQuestions,
) -> Result<(Vec<String>, Vec<crate::run::host_bind::ResolvedBind>)> {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    if let Some(effects) = q.pulled.as_ref() {
        crate::run::pull_confirm::confirm_pulled_effects(
            effects,
            q.assume_yes,
            q.interactive,
            &mut input,
            &mut std::io::stderr(),
        )?;
    }
    let denied = crate::run::host_path_consent::decide_host_paths(
        &q.origin,
        q.filesets,
        q.host_paths,
        q.assume_yes,
        q.interactive,
        &mut input,
        &mut std::io::stderr(),
    )?
    .denied;
    let binds = resolve_host_binds(q.bind_specs, q.interactive, &mut input, q.bind_decisions)?;
    Ok((denied, binds))
}

#[derive(Debug)]
struct PublishedTarget {
    image: String,
    defaults: crate::run::declarative::Defaults,
    filesets: Vec<crate::run::summary::FilesetSummary>,
    tools: Vec<String>,
    scripts: Vec<lns_ipc::SandboxScript>,
    mixins: Vec<String>,
    pinned_mixins: Vec<String>,
    contributions: Vec<lns_ipc::SourceContribution>,
}

fn published_target(
    reference: &str,
    inspection: lns_ipc::ArtifactInspection,
) -> Result<PublishedTarget> {
    match inspection {
        lns_ipc::ArtifactInspection::Sandbox(view) => {
            if view.digest.is_empty() {
                anyhow::bail!("published sandbox preflight returned no manifest digest");
            }
            let parsed: oci_client::Reference = reference
                .parse()
                .with_context(|| format!("invalid sandbox reference {reference}"))?;
            Ok(PublishedTarget {
                image: parsed.clone_with_digest(view.digest.clone()).to_string(),
                defaults: crate::run::declarative::Defaults::from_view(&view),
                filesets: crate::run::summary::fileset_summaries_from_view(&view),
                tools: crate::run::summary::tools_from_view(&view),
                scripts: view.scripts.clone(),
                mixins: view.mixins.clone(),
                pinned_mixins: view.pinned_mixins.clone(),
                contributions: view.contributions.clone(),
            })
        }
        lns_ipc::ArtifactInspection::Mixin(_) => anyhow::bail!(
            "{reference} is a published mixin, which describes no launch; run a sandbox that layers on it, or add it with --mixin"
        ),
        lns_ipc::ArtifactInspection::Image(_) => anyhow::bail!(
            "{reference} is not a sandbox; run `lns init` to author an lns.yaml, or pass a published sandbox reference"
        ),
    }
}

/// The document the preflight resolved, read the way §3.3.2 produced it: appending scripts across sources can carry a merged document past the ceilings one author is held to, and refusing here would throw away a launch the service already accepted.
fn read_resolved_definition(definition: &str) -> Result<lns_artifact::sandbox::Definition> {
    lns_artifact::sandbox::parse_resolved(definition.as_bytes())
        .context("reading the resolved definition")
}

/// A local run sends the merged document, so the mixins that produced it are already in it; a published one sends the pins its preflight showed, because the service merges them itself.
/// What a run request says about mixins: the boot merges `to_merge`, and a connector grant keys on `composed`.
struct RunMixins {
    to_merge: Vec<String>,
    composed: Vec<String>,
}

fn mixins_for_the_run(target: &crate::run::target::RunTarget, pinned: &[String]) -> RunMixins {
    RunMixins {
        to_merge: match target {
            crate::run::target::RunTarget::Local { .. } => Vec::new(),
            crate::run::target::RunTarget::Reference(_) => pinned.to_vec(),
        },
        composed: pinned.to_vec(),
    }
}

/// What the service answered for a local definition: the merged document, the sources that produced it, the egress baseline the run folds this directory's live decisions over, and which artifact carries each packed fileset they contributed.
struct ResolvedDefinition {
    definition: String,
    mixins: Vec<String>,
    pinned_mixins: Vec<String>,
    contributions: Vec<lns_ipc::SourceContribution>,
    authored_egress: String,
    packed_filesets: Vec<lns_ipc::PackedFilesetSource>,
}

/// Ask the service to resolve a local definition's mixins, since only it can pull a reference and read a directory the way the run will.
async fn preflight_local(
    socket: &Path,
    definition: &str,
    project_dir: &Path,
    mixins: &[String],
    decisions: &Path,
) -> Result<ResolvedDefinition> {
    let request = Request::ResolveDefinition {
        definition: definition.to_string(),
        project_dir: project_dir.display().to_string(),
        mixins: mixins.to_vec(),
        decisions: Some(decisions.display().to_string()),
    };
    match timeout(
        PUBLISHED_PREFLIGHT_TIMEOUT,
        real::send_request(socket, &request),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "resolving this definition's mixins timed out after {}s",
            PUBLISHED_PREFLIGHT_TIMEOUT.as_secs()
        )
    })? {
        Some(Response::DefinitionResolved {
            definition,
            mixins,
            pinned_mixins,
            contributions,
            authored_egress,
            packed_filesets,
        }) => Ok(ResolvedDefinition {
            definition,
            mixins,
            pinned_mixins,
            contributions,
            authored_egress,
            packed_filesets,
        }),
        Some(Response::Error { message }) => anyhow::bail!("{message}"),
        Some(other) => anyhow::bail!("unexpected response from daemon: {other:?}"),
        None => {
            anyhow::bail!("the service did not answer while resolving this definition's mixins")
        }
    }
}

async fn preflight_published(
    socket: &Path,
    reference: &str,
    mixins: &[String],
    decisions: &Path,
) -> Result<PublishedTarget> {
    let request = Request::InspectImage {
        image: reference.to_string(),
        mixins: mixins.to_vec(),
        decisions: Some(decisions.display().to_string()),
    };
    await_published_preflight(reference, real::send_request(socket, &request)).await
}

async fn await_published_preflight(
    reference: &str,
    response: impl std::future::Future<Output = Option<Response>>,
) -> Result<PublishedTarget> {
    let response = timeout(PUBLISHED_PREFLIGHT_TIMEOUT, response)
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "published sandbox preflight timed out after {}s while inspecting {reference}",
                PUBLISHED_PREFLIGHT_TIMEOUT.as_secs()
            )
        })?;
    match response {
        Some(Response::ImageInspected { inspection }) => published_target(reference, inspection),
        Some(Response::Error { message }) => Err(crate::service::reply::failure(&message)),
        Some(other) => anyhow::bail!("expected sandbox preflight, got {other:?}"),
        None => anyhow::bail!("no response from lns-service during sandbox preflight"),
    }
}

pub async fn run_image(
    mut args: RunArgs,
    mut target: crate::run::target::RunTarget,
    cwd: std::path::PathBuf,
    debug: bool,
) -> Result<i32> {
    let client = real_client()?;
    args.mixins = crate::run::target::root_named_directories(&args.mixins, &cwd)?;
    let project_dir = target.project_dir().unwrap_or(&cwd).to_path_buf();
    // §8.1 has every run in a directory resolve its decisions, so a directory that decided something needs the preflight even when nothing declared a mixin.
    let decisions = crate::run::summary::policy_path(&project_dir);
    let mut authored_egress = None;
    // A mixin the preflight pulled brings its filesets packed in its own artifact, and the merged document alone cannot say which.
    let mut packed_filesets = Vec::new();
    if let crate::run::target::RunTarget::Local {
        def,
        json,
        project_dir,
    } = &mut target
        && (!def.spec.mixins.is_empty() || !args.mixins.is_empty() || decisions.is_file())
    {
        let resolved =
            preflight_local(client.socket(), json, project_dir, &args.mixins, &decisions).await?;
        **def = read_resolved_definition(&resolved.definition)?;
        *json = resolved.definition;
        authored_egress = Some(resolved.authored_egress);
        packed_filesets = resolved.packed_filesets;
        crate::run::summary::adopt_pinned_mixins(
            &mut args,
            &resolved.mixins,
            &resolved.pinned_mixins,
            &resolved.contributions,
        );
    }
    let published = match &target {
        crate::run::target::RunTarget::Reference(reference) => {
            Some(preflight_published(client.socket(), reference, &args.mixins, &decisions).await?)
        }
        crate::run::target::RunTarget::Local { .. } => {
            // A path-shaped REF resolved to a definition; the summary names its image, not the path.
            if args.image.is_some() {
                args.image = Some(target.image());
            }
            None
        }
    };
    let defaults = match (&target, &published) {
        (crate::run::target::RunTarget::Local { def, .. }, _) => {
            crate::run::declarative::Defaults::from_definition(
                def,
                lns_artifact::resources::host::probe(),
            )
        }
        (_, Some(published)) => published.defaults.clone(),
        _ => crate::run::declarative::Defaults::default(),
    };
    let consumer_mount_targets: Vec<String> = args
        .mounts
        .iter()
        .map(|mount| mount.target().to_string())
        .collect();
    let resolved = crate::run::declarative::resolve(
        &defaults,
        target.project_dir().unwrap_or(&cwd),
        dirs::home_dir().as_deref(),
        args.workdir.take(),
        std::mem::take(&mut args.mounts),
    )?;
    args.workdir = resolved.workdir;
    args.mounts = resolved.mounts;
    args.publish =
        crate::run::declarative::compose_ports(&defaults.ports, std::mem::take(&mut args.publish))?;
    args.filesets = match (&target, &published) {
        (crate::run::target::RunTarget::Local { def, .. }, _) => def
            .spec
            .filesets
            .iter()
            .map(|fileset| crate::run::summary::FilesetSummary {
                source: crate::run::summary::fileset_source_display(fileset),
                guest_path: fileset.guest_path.clone(),
                owner: crate::run::summary::fileset_owner_display(fileset.owner).to_string(),
                host_path: fileset.host_path.clone(),
                optional: fileset.optional,
                declared_by: crate::run::summary::fileset_declared_by(
                    &args.contributions,
                    &fileset.guest_path,
                ),
            })
            .collect(),
        (_, Some(published)) => published.filesets.clone(),
        _ => Vec::new(),
    };
    args.tools = match (&target, &published) {
        (crate::run::target::RunTarget::Local { def, .. }, _) => def.spec.tools.clone(),
        (_, Some(published)) => published.tools.clone(),
        _ => Vec::new(),
    };
    args.scripts = match (&target, &published) {
        (crate::run::target::RunTarget::Local { def, .. }, _) => {
            crate::run::summary::script_summaries(&crate::run::summary::scripts_of(&def.spec))
        }
        (_, Some(published)) => crate::run::summary::script_summaries(&published.scripts),
        _ => Vec::new(),
    };
    // A local run resolved before this point; a published one resolves in its preflight.
    if let Some(published) = published.as_ref() {
        crate::run::summary::adopt_pinned_mixins(
            &mut args,
            &published.mixins,
            &published.pinned_mixins,
            &published.contributions,
        );
    }
    crate::run::summary::drop_overridden_mounts(&mut args, &consumer_mount_targets);
    // The size travels as its own value: writing it back into args.cpus/args.mem would tell the service the user asked for it explicitly.
    let size = crate::run::summary::resolved_size(defaults.size, &args);
    let quiet = args.quiet;
    let resolved_policy = if quiet {
        let (path, _source) = crate::run::summary::resolve_policy(&project_dir)?;
        path
    } else {
        print_run_summary(&args, size, &project_dir, &mut std::io::stderr())?
    };

    let (volumes, bind_specs) = crate::cli::split_mounts(&args.mounts);
    let interactive = host_binds_interactive(args.detach, crate::raw_mode::stdin_is_tty());
    let reference = target.image();
    let declared_mounts = published.is_some().then(|| {
        crate::run::pull_confirm::artifact_declared_mounts(&args.mounts, &consumer_mount_targets)
    });
    // A mixin is an artifact from a registry whichever document layered it in, so a local run reaches this too.
    let origin = if published.is_some() {
        crate::run::host_path_consent::DocumentOrigin::Pulled {
            reference: reference.clone(),
        }
    } else {
        crate::run::host_path_consent::DocumentOrigin::OwnDirectory
    };
    let (denied_host_paths, resolved_binds) = ask_before_boot(&PreBootQuestions {
        pulled: declared_mounts.as_ref().map(|(volumes, binds)| {
            crate::run::pull_confirm::PulledEffects {
                reference: &reference,
                binds,
                volumes,
                filesets: &args.filesets,
                tools: &args.tools,
                scripts: &args.scripts,
            }
        }),
        origin,
        filesets: &args.filesets,
        host_paths: &lns_policy::host_path_decisions::JsonFileHostPathDecisionStore::new(
            lns_ipc::host_path_decisions_path()?,
        ),
        bind_specs: &bind_specs,
        bind_decisions: &lns_policy::host_bind_decisions::JsonFileHostBindDecisionStore::new(
            lns_ipc::host_bind_decisions_path()?,
        ),
        assume_yes: args.assume_yes,
        interactive,
    })?;
    if !quiet {
        let dispositions = crate::run::summary::format_bind_dispositions(&resolved_binds);
        if !dispositions.is_empty() {
            eprint!("{dispositions}");
        }
    }
    let binds: Vec<lns_ipc::BindMount> = resolved_binds.iter().map(|b| b.to_wire()).collect();

    let socket = client.socket();
    let mut stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to {}", socket.display()))?;

    let detached = args.detach;
    let tty = !detached && args.tty && crate::raw_mode::stdin_is_tty();
    let stdin = !detached && args.interactive;
    let initial_winsize = if tty {
        crate::raw_mode::host_winsize()
    } else {
        None
    };
    let detach_chord = args.detach_keys.0.clone();
    let sandbox_user = args.effective_sandbox_user();
    let sandbox_uid = args.effective_sandbox_uid();
    let run_mixins = mixins_for_the_run(&target, &args.mixins);

    let request = Request::RunImage(Box::new(RunImageArgs {
        cpus: args.effective_cpus(),
        mem: args.effective_mem(),
        cpus_explicit: args.cpus.is_some(),
        mem_explicit: args.mem.is_some(),
        cpus_config: args.cpus_config,
        mem_config: args.mem_config,
        cmd: args.cmd,
        env: args.env,
        image: Some(target.image()),
        resolved_image: published.as_ref().map(|published| published.image.clone()),
        mixins: run_mixins.to_merge,
        composed_mixins: run_mixins.composed,
        name: args.name,
        policy_path: Some(resolved_policy.to_string_lossy().into_owned()),
        sandbox_user,
        sandbox_uid,
        entrypoint: args.entrypoint,
        hostname: args.hostname,
        workdir: args.workdir,
        debug,
        tty,
        stdin,
        initial_winsize,
        detached,
        published_ports: args.publish,
        volumes,
        binds,
        auto_remove: args.auto_remove,
        verify_sandbox: target.verify_sandbox(),
        definition: target.definition_json(),
        definition_dir: target
            .project_dir()
            .map(|p| p.to_string_lossy().into_owned()),
        authored_egress,
        packed_filesets,
        denied_host_paths,
    }));
    let frame = encode_frame(&request).context("encoding RunImage request")?;
    stream
        .write_all(&frame)
        .await
        .context("writing RunImage request")?;

    let bytes = read_frame_bytes_async(&mut stream)
        .await
        .context("reading RunStarted frame")?;
    let run_id = match decode_frame(&mut &bytes[..]).context("decoding RunStarted")? {
        Response::RunStarted { run_id } => run_id,
        Response::Error { message } => return Err(crate::service::reply::failure(&message)),
        other => anyhow::bail!("expected RunStarted, got {other:?}"),
    };

    let mut progress =
        crate::run::progress::ProgressRenderer::new(!quiet && std::io::stderr().is_terminal());
    let outcome =
        drive_pre_phase(&mut stream, &mut std::io::stderr(), &mut progress, quiet).await?;
    if let PrePhaseOutcome::EarlyExit(code) = outcome {
        return Ok(code);
    }

    render_started_run(&run_id, &mut std::io::stderr(), quiet).ok();

    if detached {
        println!("{run_id}");
        return Ok(0);
    }

    drive_attached_session(
        stream,
        Some(socket.to_path_buf()),
        run_id,
        tty,
        detach_chord,
        DetachBehaviour::DetachRun,
        quiet,
        StdinForwarding::of(stdin),
    )
    .await
}

pub async fn exec_image(args: ExecArgs) -> Result<i32> {
    if args.cmd.is_empty() {
        anyhow::bail!("lns exec requires a command");
    }

    let socket = super::socket_path()?;
    let stream = UnixStream::connect(&socket)
        .await
        .with_context(|| format!("connecting to {}", socket.display()))?;

    let initial_winsize = if args.tty {
        crate::raw_mode::host_winsize()
    } else {
        None
    };
    let mut stdout = tokio::io::stdout();
    let mut stderr = tokio::io::stderr();
    exec_image_on_stream(
        stream,
        Some(socket),
        args,
        initial_winsize,
        std::io::stdout().is_terminal(),
        &mut stdout,
        &mut stderr,
    )
    .await
}

#[allow(clippy::too_many_arguments)] // explicit stream/writer/terminal seam so the exec handshake and session loop are host-testable over in-memory duplexes
pub async fn exec_image_on_stream<S, O, E>(
    mut stream: S,
    aux_socket: Option<PathBuf>,
    args: ExecArgs,
    initial_winsize: Option<(u16, u16)>,
    stdout_is_terminal: bool,
    stdout: &mut O,
    stderr: &mut E,
) -> Result<i32>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    let tty = args.tty;
    let stdin = args.interactive;
    let detach_chord = args.detach_keys.0.clone();
    let run = args.run.clone();

    let request = Request::ExecImage(build_exec_request(
        args.run,
        args.cmd,
        tty,
        stdin,
        initial_winsize,
    ));
    let frame = encode_frame(&request).context("encoding ExecImage request")?;
    stream
        .write_all(&frame)
        .await
        .context("writing ExecImage request")?;

    let bytes = read_frame_bytes_async(&mut stream)
        .await
        .context("reading ExecStarted frame")?;
    let target = decode_exec_started(&bytes, &run)?;
    crate::log::debug!(run_id = %target.run_id(), "exec session opened");

    drive_targeted_session_with_writers(
        stream,
        aux_socket,
        target,
        tty,
        stdout_is_terminal,
        detach_chord,
        DetachBehaviour::CloseSession,
        CancelBehaviour::SignalSession,
        stdout,
        stderr,
        args.quiet,
        StdinForwarding::of(stdin),
    )
    .await
}

pub fn build_exec_request(
    run: String,
    argv: Vec<String>,
    tty: bool,
    stdin: bool,
    initial_winsize: Option<(u16, u16)>,
) -> ExecImageArgs {
    ExecImageArgs {
        run,
        argv,
        env: Vec::new(),
        tty,
        stdin,
        initial_winsize,
    }
}

fn decode_exec_started(bytes: &[u8], run: &str) -> Result<lns_ipc::SessionTarget> {
    match decode_frame(&mut &bytes[..]).context("decoding ExecStarted")? {
        Response::ExecStarted { run_id, session_id } => {
            Ok(lns_ipc::SessionTarget::Exec { run_id, session_id })
        }
        Response::Error { message } => Err(crate::service::reply::sandbox_failure(
            "exec", run, &message,
        )),
        other => anyhow::bail!("expected ExecStarted, got {other:?}"),
    }
}

/// Whether host stdin reaches the session's workload; a session opened without `-i` still watches for the detach chord but hands the run nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdinForwarding {
    ToRun,
    Withheld,
}

impl StdinForwarding {
    pub(crate) fn of(interactive: bool) -> Self {
        if interactive {
            Self::ToRun
        } else {
            Self::Withheld
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachBehaviour {
    SignalAndDrain,
    CloseSession,
    LeaveRunning,
    DetachRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CancelBehaviour {
    CancelRun,
    SignalSession,
}

#[allow(clippy::too_many_arguments)] // one session-shape argument per wire field; the writer seam lives in the _with_writers twin
pub(crate) async fn drive_attached_session<S>(
    stream: S,
    aux_socket: Option<PathBuf>,
    run_id: String,
    tty: bool,
    detach_chord: Vec<u8>,
    detach: DetachBehaviour,
    quiet: bool,
    stdin: StdinForwarding,
) -> Result<i32>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
{
    drive_targeted_session(
        stream,
        aux_socket,
        primary_target(run_id),
        tty,
        detach_chord,
        detach,
        CancelBehaviour::CancelRun,
        quiet,
        stdin,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn drive_targeted_session<S>(
    stream: S,
    aux_socket: Option<PathBuf>,
    target: lns_ipc::SessionTarget,
    tty: bool,
    detach_chord: Vec<u8>,
    detach: DetachBehaviour,
    cancel: CancelBehaviour,
    quiet: bool,
    stdin: StdinForwarding,
) -> Result<i32>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
{
    let mut stdout = tokio::io::stdout();
    let mut stderr = tokio::io::stderr();
    drive_targeted_session_with_writers(
        stream,
        aux_socket,
        target,
        tty,
        std::io::stdout().is_terminal(),
        detach_chord,
        detach,
        cancel,
        &mut stdout,
        &mut stderr,
        quiet,
        stdin,
    )
    .await
}

#[allow(clippy::too_many_arguments)] // explicit stream/writer/terminal seam so the attached-session loop is host-testable over in-memory duplexes
pub async fn drive_attached_session_with_writers<S, O, E>(
    stream: S,
    aux_socket: Option<PathBuf>,
    run_id: String,
    tty: bool,
    stdout_is_terminal: bool,
    detach_chord: Vec<u8>,
    detach: DetachBehaviour,
    stdout: &mut O,
    stderr: &mut E,
    quiet: bool,
    stdin: StdinForwarding,
) -> Result<i32>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    drive_targeted_session_with_writers(
        stream,
        aux_socket,
        primary_target(run_id),
        tty,
        stdout_is_terminal,
        detach_chord,
        detach,
        CancelBehaviour::CancelRun,
        stdout,
        stderr,
        quiet,
        stdin,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn drive_targeted_session_with_writers<S, O, E>(
    mut stream: S,
    aux_socket: Option<PathBuf>,
    target: lns_ipc::SessionTarget,
    tty: bool,
    stdout_is_terminal: bool,
    detach_chord: Vec<u8>,
    detach: DetachBehaviour,
    cancel: CancelBehaviour,
    stdout: &mut O,
    stderr: &mut E,
    quiet: bool,
    stdin: StdinForwarding,
) -> Result<i32>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    let _raw_guard = if tty {
        crate::raw_mode::RawModeGuard::enable_if_tty()
    } else {
        None
    };

    let (early_exit_tx, mut early_exit_rx) = mpsc::unbounded_channel::<()>();

    let (cancel_task, winsize_task, stdin_task) = match aux_socket {
        Some(socket) => {
            let cancel_target = target.clone();
            let cancel_socket = socket.clone();
            let cancel_task = match cancel {
                CancelBehaviour::CancelRun => {
                    let client = real_client()?;
                    tokio::spawn(async move {
                        let _ = tokio::signal::ctrl_c().await;
                        client.cancel_run(cancel_target.run_id().to_string()).await;
                    })
                }
                CancelBehaviour::SignalSession => tokio::spawn(async move {
                    let _ = tokio::signal::ctrl_c().await;
                    let _ = send_one_shot(
                        &cancel_socket,
                        &Request::SessionSignal {
                            target: cancel_target,
                            signal: SignalKind::Int,
                        },
                    )
                    .await;
                }),
            };
            let winsize_target = target.clone();
            let winsize = tty.then(|| {
                let s = socket.clone();
                tokio::spawn(async move { run_winsize_forwarder(s, winsize_target).await })
            });
            let pump_chord = detach_chord;
            let pump_early_exit = early_exit_tx.clone();
            let stdin_task = tokio::spawn(async move {
                let r = run_stdin_pump(socket, target, pump_chord, detach, stdin).await;
                if matches!(&r, Ok(true)) {
                    let _ = pump_early_exit.send(());
                }
                r
            });
            (Some(cancel_task), winsize, Some(stdin_task))
        }
        None => (None, None, None),
    };

    let mut last_stdout_byte: Option<u8> = None;

    let exit_code = loop {
        tokio::select! {
            biased;
            _ = early_exit_rx.recv() => {
                break exit_code_after_detach(
                    detach,
                    tty,
                    &mut stream,
                    stdout,
                    stderr,
                    &mut last_stdout_byte,
                    quiet,
                )
                .await;
            }
            res = read_frame_bytes_async(&mut stream) => {
                let bytes = res.context("reading run-response frame")?;
                let wire = decode_wire_frame_from_bytes(&bytes).context("decoding run-response")?;
                match wire {
                    WireFrame::Stdout(b) => {
                        last_stdout_byte = b.last().copied().or(last_stdout_byte);
                        stdout.write_all(&b).await?;
                        stdout.flush().await.ok();
                    }
                    WireFrame::Stderr(b) => {
                        stderr.write_all(&b).await?;
                        stderr.flush().await.ok();
                    }
                    WireFrame::Json(Response::RunLog { level, verb, message }) => {
                        render_attached_run_log(level, verb.as_deref(), &message, tty, stderr, quiet)
                            .await?;
                    }
                    WireFrame::Json(Response::RunProgress { .. }) => {}
                    WireFrame::Json(Response::RunExit { code }) => break code,
                    WireFrame::Json(Response::Error { message }) => {
                        return Err(crate::service::reply::failure(&message))
                    }
                    other => anyhow::bail!("unexpected response from daemon: {other:?}"),
                }
            }
        }
    };

    if !tty && stdout_is_terminal && needs_final_newline(last_stdout_byte) {
        stdout.write_all(b"\n").await.ok();
        stdout.flush().await.ok();
    }

    if let Some(t) = stdin_task {
        t.abort();
    }
    if let Some(t) = winsize_task {
        t.abort();
    }
    if let Some(t) = cancel_task {
        t.abort();
    }
    Ok(exit_code)
}

fn needs_final_newline(last_byte: Option<u8>) -> bool {
    matches!(last_byte, Some(b) if b != b'\n')
}

async fn render_attached_run_log<E>(
    level: LogLevel,
    verb: Option<&str>,
    message: &str,
    tty: bool,
    stderr: &mut E,
    quiet: bool,
) -> Result<()>
where
    E: AsyncWriteExt + Unpin,
{
    if matches!(level, LogLevel::Debug) {
        crate::log::debug!("{message}");
        return Ok(());
    }
    let mut line = Vec::<u8>::new();
    render_status_line(level, verb, message, &mut line, quiet)?;
    if line.is_empty() {
        return Ok(());
    }
    if tty {
        line = lf_to_crlf(&line);
    }
    stderr.write_all(&line).await?;
    stderr.flush().await.ok();
    Ok(())
}

fn lf_to_crlf(line: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len() + 2);
    for &b in line {
        if b == b'\n' {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

async fn exit_code_after_detach<S, O, E>(
    detach: DetachBehaviour,
    tty: bool,
    stream: &mut S,
    stdout: &mut O,
    stderr: &mut E,
    last_stdout_byte: &mut Option<u8>,
    quiet: bool,
) -> i32
where
    S: AsyncReadExt + Unpin,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    match detach {
        DetachBehaviour::SignalAndDrain => {
            drain_after_chord(stream, tty, stdout, stderr, last_stdout_byte, quiet).await
        }
        DetachBehaviour::CloseSession
        | DetachBehaviour::LeaveRunning
        | DetachBehaviour::DetachRun => 0,
    }
}

async fn drain_after_chord<S, O, E>(
    stream: &mut S,
    tty: bool,
    stdout: &mut O,
    stderr: &mut E,
    last_stdout_byte: &mut Option<u8>,
    quiet: bool,
) -> i32
where
    S: AsyncReadExt + Unpin,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    match timeout(
        Duration::from_secs(5),
        drain_to_exit(stream, tty, stdout, stderr, last_stdout_byte, quiet),
    )
    .await
    {
        Ok(Ok(code)) => code,
        Ok(Err(e)) => {
            crate::log::debug!("post-chord drain error: {e}");
            0
        }
        Err(_) => {
            crate::log::debug!("post-chord drain timed out; returning 0");
            0
        }
    }
}

async fn drain_to_exit<S, O, E>(
    stream: &mut S,
    tty: bool,
    stdout: &mut O,
    stderr: &mut E,
    last_stdout_byte: &mut Option<u8>,
    quiet: bool,
) -> Result<i32>
where
    S: AsyncReadExt + Unpin,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    loop {
        let bytes = match read_frame_bytes_async(stream).await {
            Ok(b) => b,
            Err(_) => return Ok(0),
        };
        let wire = decode_wire_frame_from_bytes(&bytes).context("decoding drain-frame")?;
        match wire {
            WireFrame::Stdout(b) => {
                *last_stdout_byte = b.last().copied().or(*last_stdout_byte);
                let _ = stdout.write_all(&b).await;
            }
            WireFrame::Stderr(b) => {
                let _ = stderr.write_all(&b).await;
            }
            WireFrame::Json(Response::RunLog {
                level,
                verb,
                message,
            }) => {
                render_attached_run_log(level, verb.as_deref(), &message, tty, stderr, quiet)
                    .await?;
            }
            WireFrame::Json(Response::RunExit { code }) => return Ok(code),
            WireFrame::Json(Response::Error { message }) => {
                crate::log::debug!("drain error from daemon: {message}");
                return Ok(0);
            }
            _ => {}
        }
    }
}

async fn run_stdin_pump(
    socket: PathBuf,
    target: lns_ipc::SessionTarget,
    detach_chord: Vec<u8>,
    detach: DetachBehaviour,
    stdin: StdinForwarding,
) -> Result<bool> {
    let mut input = tokio::io::stdin();
    let mut buf = [0u8; 4096];
    let mut detector = (!detach_chord.is_empty()).then(|| DetachChordDetector::new(detach_chord));
    loop {
        let n = match input.read(&mut buf).await {
            Ok(0) => {
                if let Some(d) = detector.as_mut() {
                    let held = d.drain_for_eof();
                    if !held.is_empty() && stdin == StdinForwarding::ToRun {
                        let _ = send_one_shot(
                            &socket,
                            &Request::SessionStdin {
                                target: target.clone(),
                                bytes: held,
                            },
                        )
                        .await;
                    }
                }
                if stdin == StdinForwarding::ToRun {
                    let _ = send_one_shot(
                        &socket,
                        &Request::SessionStdinClose {
                            target: target.clone(),
                        },
                    )
                    .await;
                }
                return Ok(false);
            }
            Ok(n) => n,
            Err(e) => {
                crate::log::debug!("stdin read error: {e}");
                return Err(e.into());
            }
        };
        let chunk = &buf[..n];
        match detector.as_mut() {
            Some(d) => {
                if !pump_with_detector_target(&socket, &target, chunk, d, detach, stdin).await? {
                    return Ok(true);
                }
            }
            None => {
                if stdin == StdinForwarding::ToRun {
                    send_one_shot(
                        &socket,
                        &Request::SessionStdin {
                            target: target.clone(),
                            bytes: chunk.to_vec(),
                        },
                    )
                    .await?;
                }
            }
        }
    }
}

async fn pump_with_detector_target(
    socket: &Path,
    target: &lns_ipc::SessionTarget,
    bytes: &[u8],
    detector: &mut DetachChordDetector,
    detach: DetachBehaviour,
    stdin: StdinForwarding,
) -> Result<bool> {
    let mut pending: Vec<u8> = Vec::new();
    for &b in bytes {
        let (requests, control) =
            plan_feed_target(detector.feed(b), target, &mut pending, detach, stdin);
        for request in &requests {
            send_one_shot(socket, request).await?;
        }
        if matches!(control, PumpControl::Detach) {
            return Ok(false);
        }
    }
    if stdin == StdinForwarding::ToRun
        && let Some(request) = drain_pending_target(target, &mut pending)
    {
        send_one_shot(socket, &request).await?;
    }
    Ok(true)
}

enum PumpControl {
    Continue,
    Detach,
}

fn drain_pending_target(target: &lns_ipc::SessionTarget, pending: &mut Vec<u8>) -> Option<Request> {
    if pending.is_empty() {
        None
    } else {
        Some(Request::SessionStdin {
            target: target.clone(),
            bytes: std::mem::take(pending),
        })
    }
}

#[cfg(test)]
fn plan_feed(
    action: FeedAction,
    run_id: &str,
    pending: &mut Vec<u8>,
    detach: DetachBehaviour,
    stdin: StdinForwarding,
) -> (Vec<Request>, PumpControl) {
    plan_feed_target(
        action,
        &primary_target(run_id.to_string()),
        pending,
        detach,
        stdin,
    )
}

fn plan_feed_target(
    action: FeedAction,
    target: &lns_ipc::SessionTarget,
    pending: &mut Vec<u8>,
    detach: DetachBehaviour,
    stdin: StdinForwarding,
) -> (Vec<Request>, PumpControl) {
    let stdin_requests = |bytes: Vec<u8>| match stdin {
        StdinForwarding::ToRun => vec![Request::SessionStdin {
            target: target.clone(),
            bytes,
        }],
        StdinForwarding::Withheld => Vec::new(),
    };
    let drained = |pending: &mut Vec<u8>| match stdin {
        StdinForwarding::ToRun => drain_pending_target(target, pending).into_iter().collect(),
        StdinForwarding::Withheld => {
            pending.clear();
            Vec::<Request>::new()
        }
    };
    match action {
        FeedAction::Forward(byte) => {
            pending.push(byte);
            (Vec::new(), PumpControl::Continue)
        }
        FeedAction::Hold => (drained(pending), PumpControl::Continue),
        FeedAction::Flush(held) => {
            let mut requests = drained(pending);
            requests.extend(stdin_requests(held));
            (requests, PumpControl::Continue)
        }
        FeedAction::FlushAndForward(held, current) => {
            let mut requests = drained(pending);
            let mut combined = held;
            combined.push(current);
            requests.extend(stdin_requests(combined));
            (requests, PumpControl::Continue)
        }
        FeedAction::Trigger => {
            let mut requests = drained(pending);
            match detach {
                DetachBehaviour::SignalAndDrain => requests.push(Request::SessionSignal {
                    target: target.clone(),
                    signal: SignalKind::Hup,
                }),
                DetachBehaviour::CloseSession => requests.push(Request::SessionDetach {
                    target: target.clone(),
                }),
                DetachBehaviour::DetachRun => requests.push(Request::SessionDetach {
                    target: target.clone(),
                }),
                DetachBehaviour::LeaveRunning => {}
            }
            (requests, PumpControl::Detach)
        }
    }
}

async fn run_winsize_forwarder(socket: PathBuf, target: lns_ipc::SessionTarget) -> Result<()> {
    use tokio::signal::unix::{SignalKind as TokioSig, signal};
    let mut sigwinch = match signal(TokioSig::window_change()) {
        Ok(s) => s,
        Err(e) => {
            crate::log::warn!("SIGWINCH watcher not installed: {e}");
            return Ok(());
        }
    };
    while sigwinch.recv().await.is_some() {
        let Some((rows, cols)) = crate::raw_mode::host_winsize() else {
            continue;
        };
        if send_one_shot(
            &socket,
            &Request::SessionResize {
                target: target.clone(),
                rows,
                cols,
            },
        )
        .await
        .is_err()
        {
            return Ok(());
        }
    }
    Ok(())
}

async fn send_one_shot(socket: &Path, request: &Request) -> Result<()> {
    let mut stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to {} for one-shot", socket.display()))?;
    write_and_await_ack(&mut stream, request).await
}

async fn write_and_await_ack<S>(stream: &mut S, request: &Request) -> Result<()>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let frame = encode_frame(request).context("encoding one-shot request")?;
    stream
        .write_all(&frame)
        .await
        .context("writing one-shot request")?;
    // Awaiting the reply is load-bearing for RunDetach: it blocks until the service has fired detach_tx, so the CLI never closes the run stream first and trips WriteFailed → CancelAndDeregister.
    let _ = read_frame_bytes_async(stream).await;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrePhaseOutcome {
    SessionReady,
    EarlyExit(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrePhaseStep {
    Continue,
    SessionReady,
    EarlyExit(i32),
}

pub fn pre_phase_step<W>(
    bytes: &[u8],
    writer: &mut W,
    progress: &mut crate::run::progress::ProgressRenderer,
    quiet: bool,
) -> Result<PrePhaseStep>
where
    W: std::io::Write,
{
    let wire = decode_wire_frame_from_bytes(bytes).context("decoding pre-phase frame")?;
    if let WireFrame::Json(Response::RunProgress {
        verb,
        message,
        current,
        total,
    }) = wire
    {
        progress.update(&verb, &message, current, total, writer)?;
        return Ok(PrePhaseStep::Continue);
    }
    progress.clear(writer)?;
    match wire {
        WireFrame::Json(Response::RunLog {
            level,
            verb,
            message,
        }) => {
            render_status_line(level, verb.as_deref(), &message, writer, quiet)?;
            if verb.as_deref() == Some("SessionReady") {
                Ok(PrePhaseStep::SessionReady)
            } else {
                Ok(PrePhaseStep::Continue)
            }
        }
        WireFrame::Json(Response::OauthVerification {
            verification_uri,
            user_code,
            expires_in_secs,
        }) => {
            writeln!(
                writer,
                "Open {verification_uri} and enter code {user_code} (expires in {}m)",
                expires_in_secs / 60
            )?;
            Ok(PrePhaseStep::Continue)
        }
        WireFrame::Json(Response::OauthBrowserOpened { authorization_url }) => {
            writeln!(
                writer,
                "Opening your browser to authorize… (if it didn't open, visit {authorization_url})"
            )?;
            Ok(PrePhaseStep::Continue)
        }
        WireFrame::Json(Response::RunExit { .. }) => Ok(PrePhaseStep::EarlyExit(PRE_START_FAILURE)),
        WireFrame::Json(Response::Error { message }) => {
            Err(crate::service::reply::failure(&message))
        }
        other => anyhow::bail!("unexpected frame before SessionReady: {other:?}"),
    }
}

const PROGRESS_TICK: Duration = Duration::from_millis(100);

pub async fn drive_pre_phase<S, W>(
    stream: &mut S,
    writer: &mut W,
    progress: &mut crate::run::progress::ProgressRenderer,
    quiet: bool,
) -> Result<PrePhaseOutcome>
where
    S: AsyncReadExt + Unpin,
    W: std::io::Write,
{
    let mut ticker = tokio::time::interval(PROGRESS_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let read = read_frame_bytes_async(stream);
        tokio::pin!(read);
        let bytes = loop {
            tokio::select! {
                r = &mut read => break r,
                _ = ticker.tick() => progress.tick(writer)?,
            }
        }
        .context("reading pre-phase frame")?;
        match pre_phase_step(&bytes, writer, progress, quiet)? {
            PrePhaseStep::Continue => {}
            PrePhaseStep::SessionReady => return Ok(PrePhaseOutcome::SessionReady),
            PrePhaseStep::EarlyExit(code) => return Ok(PrePhaseOutcome::EarlyExit(code)),
        }
    }
}

pub fn render_status_line(
    level: LogLevel,
    verb: Option<&str>,
    message: &str,
    writer: &mut impl std::io::Write,
    quiet: bool,
) -> std::io::Result<()> {
    if quiet && !matches!(level, LogLevel::Warn | LogLevel::Error) {
        return Ok(());
    }
    let marker = match level {
        LogLevel::Error => '✗',
        LogLevel::Warn => '⚠',
        _ => '✓',
    };
    let phrase = phrase_for_verb(verb.unwrap_or(""));
    if message.is_empty() {
        writeln!(writer, "{marker} {phrase}")
    } else {
        writeln!(writer, "{marker} {phrase} {message}")
    }
}

pub fn render_started_run(
    run_id: &str,
    writer: &mut impl std::io::Write,
    quiet: bool,
) -> std::io::Result<()> {
    render_status_line(
        LogLevel::Info,
        Some("Started"),
        &format!("run {}", lns_ipc::short_run_id(run_id)),
        writer,
        quiet,
    )
}

fn phrase_for_verb(verb: &str) -> String {
    match verb {
        "SessionReady" => "session ready".to_string(),
        "ImageCached" => "image cached".to_string(),
        _ => verb.to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved_definition_with_scripts(count: usize) -> String {
        let entries: Vec<String> = (0..count)
            .map(|n| format!(r#"{{"when":"pre-start","run":"echo {n}"}}"#))
            .collect();
        format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{{"image":"registry.example.test/runtime:1","scripts":[{}]}}}}"#,
            entries.join(",")
        )
    }

    #[test]
    fn the_resolved_definition_keeps_every_script_the_merge_appended() {
        let beyond = lns_artifact::sandbox::MAX_SCRIPT_STEPS + 1;
        let def = read_resolved_definition(&resolved_definition_with_scripts(beyond)).expect(
            "the service already accepted this merge, so reading it back must not refuse the launch",
        );
        assert_eq!(
            def.spec.scripts.len(),
            beyond,
            "the ceiling bounds what one author may declare; a sum produced by appending across sources is nobody's mistake to correct, and refusing it here would discard a launch the preflight approved"
        );
    }

    #[test]
    fn a_resolved_definition_that_is_not_a_sandbox_is_still_refused() {
        let err = read_resolved_definition(
            r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{}}"#,
        )
        .expect_err("the document that boots still needs the block that names what boots");
        assert!(
            format!("{err:#}").contains("image"),
            "relaxing the authoring ceilings must not relax everything else; got: {err:#}"
        );
    }

    #[test]
    fn exec_started_handshake_preserves_the_service_assigned_session_target() {
        let frame = encode_frame(&Response::ExecStarted {
            run_id: "run-7".to_string(),
            session_id: "exec-2".to_string(),
        })
        .unwrap();

        assert_eq!(
            decode_exec_started(&frame, "7").unwrap(),
            lns_ipc::SessionTarget::Exec {
                run_id: "run-7".to_string(),
                session_id: "exec-2".to_string(),
            }
        );
    }
    use tokio::io::AsyncWriteExt;

    #[test]
    fn a_local_run_sends_no_mixins_because_its_document_already_carries_them() {
        let pinned = vec!["/work/mixins/pg".to_string()];
        let local = crate::run::target::RunTarget::Local {
            def: Box::new(
                lns_artifact::sandbox::parse(
                    br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"x:1"}}"#,
                )
                .expect("the fixture is a valid definition"),
            ),
            json: String::new(),
            project_dir: std::path::PathBuf::from("/work"),
        };
        assert!(
            mixins_for_the_run(&local, &pinned).to_merge.is_empty(),
            "the local preflight already merged them, so sending them again asks the service to merge a document it was not given"
        );
        assert_eq!(
            mixins_for_the_run(
                &crate::run::target::RunTarget::Reference("ghcr.io/acme/agent:1".into()),
                &pinned
            )
            .to_merge,
            pinned,
            "a published run merges service-side, so the boot needs the pins its preflight showed"
        );
        assert_eq!(
            mixins_for_the_run(&local, &pinned).composed,
            pinned,
            "a local run is still composed of what it merged, and a connector grant keys on that — sending nothing here spends the bare run's grant on a mixin the user added"
        );
    }

    #[test]
    fn published_sandbox_preflight_pins_the_artifact_and_keeps_launch_defaults() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let target = published_target(
            "registry.example.test/team/sandbox:1",
            lns_ipc::ArtifactInspection::Sandbox(Box::new(lns_ipc::SandboxView {
                mixins: vec!["ghcr.io/acme/postgres-tools@sha256:c41e8b7d".into()],
                pinned_mixins: vec!["ghcr.io/acme/obs@sha256:5b9e1f0a".into()],
                contributions: Vec::new(),
                reference: "registry.example.test/team/sandbox:1".into(),
                digest: digest.clone(),
                image: "registry.example.test/runtime:1".into(),
                workdir: Some("/workspace".into()),
                user: None,
                mounts: vec![lns_ipc::SandboxMount {
                    kind: lns_ipc::SandboxMountKind::Bind,
                    source: ".".into(),
                    target: "/workspace".into(),
                    read_only: false,
                    exclude: Vec::new(),
                    optional: false,
                    size_bytes: None,
                }],
                ports: Vec::new(),
                filesets: vec![lns_ipc::SandboxFileset {
                    path: Some("./skills".into()),
                    inline: false,
                    host_path: None,
                    optional: false,
                    guest_path: "/root/.agent/skills".into(),
                    owner: lns_ipc::SandboxFilesetOwner::Workload,
                }],
                connectors: Vec::new(),
                env: Vec::new(),
                credentials: Vec::new(),
                tools: Vec::new(),
                scripts: Vec::new(),
                policy_flags: Vec::new(),
                cpus: None,
                mem_mib: None,
                disk_bytes: None,
            })),
        )
        .unwrap();
        assert_eq!(
            target.image,
            format!("registry.example.test/team/sandbox@{digest}")
        );
        assert_eq!(target.defaults.workdir.as_deref(), Some("/workspace"));
        assert_eq!(target.defaults.mounts[0].source, ".");
        assert_eq!(
            target.filesets,
            [crate::run::summary::FilesetSummary {
                source: "./skills".to_string(),
                guest_path: "/root/.agent/skills".to_string(),
                owner: "workload".to_string(),
                host_path: None,
                optional: false,
                declared_by: None,
            }]
        );
        assert_eq!(
            target.pinned_mixins,
            ["ghcr.io/acme/obs@sha256:5b9e1f0a"],
            "the run boots the digest its preflight pinned, and this is the only hop that carries it there"
        );
        assert_eq!(
            target.mixins,
            ["ghcr.io/acme/postgres-tools@sha256:c41e8b7d"],
            "the run summary names what a composed sandbox resolved into, and this is the only hop that carries it there"
        );
    }

    #[test]
    fn published_preflight_refuses_a_plain_image() {
        let err = published_target(
            "registry.example.test/team/image:1",
            lns_ipc::ArtifactInspection::Image(lns_ipc::ImageView {
                reference: "registry.example.test/team/image:1".into(),
                digest: format!("sha256:{}", "a".repeat(64)),
            }),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("not a sandbox"));
    }

    #[test]
    fn published_sandbox_preflight_refuses_an_unpinned_result() {
        let err = published_target(
            "registry.example.test/team/sandbox:1",
            lns_ipc::ArtifactInspection::Sandbox(Box::new(lns_ipc::SandboxView {
                mixins: Vec::new(),
                pinned_mixins: Vec::new(),
                contributions: Vec::new(),
                reference: "registry.example.test/team/sandbox:1".into(),
                digest: String::new(),
                image: "registry.example.test/runtime:1".into(),
                workdir: Some("/workspace".into()),
                user: None,
                mounts: Vec::new(),
                ports: Vec::new(),
                filesets: Vec::new(),
                connectors: Vec::new(),
                env: Vec::new(),
                credentials: Vec::new(),
                tools: Vec::new(),
                scripts: Vec::new(),
                policy_flags: Vec::new(),
                cpus: None,
                mem_mib: None,
                disk_bytes: None,
            })),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("no manifest digest"));
    }

    #[tokio::test(start_paused = true)]
    async fn published_preflight_times_out_when_the_service_never_responds() {
        let err = await_published_preflight(
            "registry.example.test/team/sandbox:1",
            std::future::pending::<Option<Response>>(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "published sandbox preflight timed out after 30s while inspecting registry.example.test/team/sandbox:1"
        );
    }

    #[tokio::test]
    async fn drive_attached_session_returns_exit_code_from_run_exit_frame() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let frame = encode_frame(&Response::RunExit { code: 7 }).expect("encode RunExit");
            server.write_all(&frame).await.expect("write RunExit");
        });
        let code = drive_attached_session(
            client,
            None,
            "2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a".to_string(),
            false,
            Vec::new(),
            DetachBehaviour::SignalAndDrain,
            false,
            StdinForwarding::ToRun,
        )
        .await
        .expect("drive_attached_session");
        assert_eq!(code, 7);
    }

    fn run_log(verb: &str, message: &str) -> Response {
        Response::RunLog {
            level: LogLevel::Info,
            verb: Some(verb.to_string()),
            message: message.to_string(),
        }
    }

    #[test]
    fn render_pre_phase_log_emits_check_marker_and_lowercased_verb() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Info,
            Some("Resolved"),
            "ubuntu:latest",
            &mut buf,
            false,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓ resolved ubuntu:latest\n");
    }

    #[test]
    fn render_pre_phase_log_uses_cross_marker_for_error_level() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Error,
            Some("Resolve"),
            "registry timeout",
            &mut buf,
            false,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✗ resolve registry timeout\n");
    }

    #[test]
    fn render_pre_phase_log_special_cases_session_ready_phrase() {
        let mut buf = Vec::<u8>::new();
        render_status_line(LogLevel::Info, Some("SessionReady"), "", &mut buf, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓ session ready\n");
    }

    #[test]
    fn render_pre_phase_log_special_cases_image_cached_phrase() {
        let mut buf = Vec::<u8>::new();
        render_status_line(LogLevel::Info, Some("ImageCached"), "", &mut buf, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓ image cached\n");
    }

    #[test]
    fn render_pre_phase_log_omits_trailing_space_when_message_empty() {
        let mut buf = Vec::<u8>::new();
        render_status_line(LogLevel::Info, Some("Booted"), "", &mut buf, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓ booted\n");
    }

    #[test]
    fn render_pre_phase_log_emits_no_ansi_escape_sequences() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Info,
            Some("Booted"),
            "microVM (1.1s)",
            &mut buf,
            false,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(!s.contains('\x1b'), "ANSI escape leaked: {s:?}");
    }

    #[test]
    fn render_pre_phase_log_falls_back_to_lowercased_verb_for_unknown_verbs() {
        let mut buf = Vec::<u8>::new();
        render_status_line(LogLevel::Info, None, "unverbed", &mut buf, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓  unverbed\n");
    }

    async fn write_response(server: &mut tokio::io::DuplexStream, resp: Response) {
        let frame = encode_frame(&resp).expect("encode");
        server.write_all(&frame).await.expect("write");
    }

    fn no_progress() -> crate::run::progress::ProgressRenderer {
        crate::run::progress::ProgressRenderer::new(false)
    }

    fn run_progress(verb: &str, message: &str, current: u64, total: u64) -> Response {
        Response::RunProgress {
            verb: verb.to_string(),
            message: message.to_string(),
            current,
            total,
        }
    }

    #[tokio::test]
    async fn drive_pre_phase_renders_log_frames_and_returns_session_ready() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(&mut server, run_log("Resolved", "ubuntu:latest")).await;
            write_response(&mut server, run_log("Pulled", "7 layers")).await;
            write_response(&mut server, run_log("Booted", "microVM")).await;
            write_response(&mut server, run_log("SessionReady", "")).await;
        });
        let mut buf = Vec::<u8>::new();
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), false)
            .await
            .unwrap();
        assert_eq!(outcome, PrePhaseOutcome::SessionReady);
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains("✓ resolved ubuntu:latest"),
            "missing resolved: {s}"
        );
        assert!(s.contains("✓ pulled 7 layers"), "missing pulled: {s}");
        assert!(s.contains("✓ booted microVM"), "missing booted: {s}");
        assert!(s.contains("✓ session ready"), "missing session ready: {s}");
    }

    #[tokio::test]
    async fn a_run_exit_before_session_ready_is_lns_failing_never_the_workload() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(&mut server, run_log("Resolving", "ubuntu:latest")).await;
            write_response(&mut server, Response::RunExit { code: 42 }).await;
        });
        let mut buf = Vec::<u8>::new();
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), false)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            PrePhaseOutcome::EarlyExit(PRE_START_FAILURE),
            "the workload never started, so its encoded code must not leak through as a workload status"
        );
    }

    #[tokio::test]
    async fn drive_pre_phase_bubbles_up_daemon_error_before_session_ready() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(
                &mut server,
                Response::Error {
                    message: "no such image".to_string(),
                },
            )
            .await;
        });
        let mut buf = Vec::<u8>::new();
        let err = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), false)
            .await
            .expect_err("daemon Error must surface");
        assert!(format!("{err:#}").contains("no such image"));
    }

    #[tokio::test]
    async fn drive_pre_phase_rejects_stdout_frames_before_session_ready() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let frame = lns_ipc::encode_wire_frame(&WireFrame::Stdout(b"early bytes".to_vec()))
                .expect("encode wire frame");
            server.write_all(&frame).await.expect("write");
        });
        let mut buf = Vec::<u8>::new();
        let err = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), false)
            .await
            .expect_err("stdout before SessionReady is a protocol violation");
        assert!(format!("{err:#}").contains("unexpected frame"));
    }

    #[tokio::test]
    async fn drive_pre_phase_renders_a_device_sign_in_prompt_and_continues() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(
                &mut server,
                Response::OauthVerification {
                    verification_uri: "https://api.some-oauth.example/device".into(),
                    user_code: "SOME-CODE".into(),
                    expires_in_secs: 900,
                },
            )
            .await;
            write_response(&mut server, run_log("SessionReady", "")).await;
        });
        let mut buf = Vec::<u8>::new();
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), false)
            .await
            .unwrap();
        assert_eq!(outcome, PrePhaseOutcome::SessionReady);
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains("Open https://api.some-oauth.example/device and enter code SOME-CODE (expires in 15m)"),
            "got: {s}"
        );
    }

    #[tokio::test]
    async fn drive_pre_phase_renders_a_browser_sign_in_line_and_continues() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(
                &mut server,
                Response::OauthBrowserOpened {
                    authorization_url: "https://api.some-oauth.example/authorize".into(),
                },
            )
            .await;
            write_response(&mut server, run_log("SessionReady", "")).await;
        });
        let mut buf = Vec::<u8>::new();
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), false)
            .await
            .unwrap();
        assert_eq!(outcome, PrePhaseOutcome::SessionReady);
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains("visit https://api.some-oauth.example/authorize"),
            "got: {s}"
        );
    }

    #[tokio::test]
    async fn drive_pre_phase_ignores_progress_frames_when_stderr_is_not_a_tty() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(&mut server, run_progress("Pulling", "", 5, 10)).await;
            write_response(&mut server, run_progress("Booting", "microVM", 0, 0)).await;
            write_response(&mut server, run_log("SessionReady", "")).await;
        });
        let mut buf = Vec::<u8>::new();
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), false)
            .await
            .unwrap();
        assert_eq!(outcome, PrePhaseOutcome::SessionReady);
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(
            s, "✓ session ready\n",
            "piped output must contain only final status lines",
        );
    }

    #[tokio::test]
    async fn drive_pre_phase_renders_then_clears_progress_before_the_next_status_line() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(&mut server, run_progress("Pulling", "", 5, 10)).await;
            write_response(&mut server, run_log("Pulled", "2 layers")).await;
            write_response(&mut server, run_log("SessionReady", "")).await;
        });
        let mut buf = Vec::<u8>::new();
        let mut progress = crate::run::progress::ProgressRenderer::new(true);
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut progress, false)
            .await
            .unwrap();
        assert_eq!(outcome, PrePhaseOutcome::SessionReady);
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("pulling"), "live bar must render: {s:?}");
        assert!(s.contains("50%"), "{s:?}");
        let check_idx = s.find('✓').unwrap();
        assert!(
            s[..check_idx].ends_with('\r'),
            "the bar must be erased so the status line starts at column 0: {s:?}",
        );
        assert!(s.contains("✓ pulled 2 layers"), "{s:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn drive_pre_phase_animates_the_spinner_while_waiting_between_frames() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(&mut server, run_progress("Booting", "microVM", 0, 0)).await;
            tokio::time::sleep(Duration::from_millis(350)).await;
            write_response(&mut server, run_log("SessionReady", "")).await;
        });
        let mut buf = Vec::<u8>::new();
        let mut progress = crate::run::progress::ProgressRenderer::new(true);
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut progress, false)
            .await
            .unwrap();
        assert_eq!(outcome, PrePhaseOutcome::SessionReady);
        let s = String::from_utf8(buf).unwrap();
        let spinner_glyphs: std::collections::HashSet<char> = s
            .chars()
            .filter(|c| ('\u{2800}'..='\u{28FF}').contains(c))
            .collect();
        assert!(
            spinner_glyphs.len() >= 2,
            "the spinner must visibly animate while the service is silent: {s:?}",
        );
        assert!(s.contains("✓ session ready"), "{s:?}");
    }

    #[tokio::test]
    async fn drive_attached_session_tolerates_progress_frames_after_session_ready() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let progress = encode_frame(&run_progress("Pulling", "", 1, 2)).expect("encode");
            server.write_all(&progress).await.expect("write progress");
            let exit = encode_frame(&Response::RunExit { code: 0 }).expect("encode RunExit");
            server.write_all(&exit).await.expect("write RunExit");
        });
        let code = drive_attached_session(
            client,
            None,
            "2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a".to_string(),
            false,
            Vec::new(),
            DetachBehaviour::SignalAndDrain,
            false,
            StdinForwarding::ToRun,
        )
        .await
        .expect("a stray progress frame must not kill an attached session");
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn drive_attached_session_bubbles_up_a_daemon_error_frame() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let frame = encode_frame(&Response::Error {
                message: "policy file is unreadable".to_string(),
            })
            .expect("encode Error");
            server.write_all(&frame).await.expect("write Error");
        });
        let err = drive_attached_session(
            client,
            None,
            "2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a".to_string(),
            false,
            Vec::new(),
            DetachBehaviour::SignalAndDrain,
            false,
            StdinForwarding::ToRun,
        )
        .await
        .expect_err("daemon Error must surface as anyhow::Error");
        assert!(
            format!("{err:#}").contains("policy file is unreadable"),
            "error context lost"
        );
    }

    #[test]
    fn render_status_line_renders_started_run_in_check_cadence_not_right_aligned() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Info,
            Some("Started"),
            "run 2a2a2a2a2a2a",
            &mut buf,
            false,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓ started run 2a2a2a2a2a2a\n");
        assert!(
            !s.contains("     Started"),
            "right-aligned padding leaked: {s:?}"
        );
    }

    #[test]
    fn render_started_run_routes_run_id_through_the_check_renderer() {
        let mut buf = Vec::<u8>::new();
        render_started_run("07eeeeee000000000000000000000000", &mut buf, false).unwrap();
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "✓ started run 07eeeeee0000\n"
        );
    }

    #[test]
    fn render_status_line_renders_finished_in_check_cadence() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Info,
            Some("Finished"),
            "in 2.53s",
            &mut buf,
            false,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓ finished in 2.53s\n");
    }

    #[test]
    fn render_status_line_uses_cross_marker_for_error_level() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Error,
            Some("Resolve"),
            "registry timeout",
            &mut buf,
            false,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✗ resolve registry timeout\n");
    }

    fn render_marker(level: LogLevel) -> char {
        let mut buf = Vec::<u8>::new();
        render_status_line(level, Some("Verb"), "msg", &mut buf, false).unwrap();
        String::from_utf8(buf)
            .unwrap()
            .chars()
            .next()
            .expect("status line has a leading marker")
    }

    #[test]
    fn render_status_line_marker_truth_table_distinguishes_each_level() {
        assert_eq!(render_marker(LogLevel::Info), '✓');
        assert_eq!(
            render_marker(LogLevel::Warn),
            '⚠',
            "a warning must not wear the success check",
        );
        assert_eq!(render_marker(LogLevel::Error), '✗');
    }

    #[test]
    fn render_status_line_phrases_session_ready() {
        let mut buf = Vec::<u8>::new();
        render_status_line(LogLevel::Info, Some("SessionReady"), "", &mut buf, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓ session ready\n");
        assert!(!s.contains("SessionReady"), "raw verb leaked: {s:?}");
    }

    #[tokio::test]
    async fn render_attached_run_log_keeps_debug_frames_off_the_user_writer() {
        let mut stderr = Vec::<u8>::new();
        render_attached_run_log(
            LogLevel::Debug,
            None,
            "broker handshake",
            false,
            &mut stderr,
            false,
        )
        .await
        .expect("render debug");
        assert!(
            stderr.is_empty(),
            "debug run-log must not reach the user's status writer: {stderr:?}",
        );
    }

    #[tokio::test]
    async fn render_attached_run_log_writes_info_status_to_the_writer() {
        let mut stderr = Vec::<u8>::new();
        render_attached_run_log(
            LogLevel::Info,
            Some("Finished"),
            "in 1.10s",
            false,
            &mut stderr,
            false,
        )
        .await
        .expect("render info");
        assert_eq!(stderr, b"\xe2\x9c\x93 finished in 1.10s\n");
    }

    #[tokio::test]
    async fn render_attached_run_log_uses_crlf_when_the_terminal_is_raw() {
        let mut stderr = Vec::<u8>::new();
        render_attached_run_log(
            LogLevel::Warn,
            None,
            "workload exited with code 1",
            true,
            &mut stderr,
            false,
        )
        .await
        .expect("render warn");
        let rendered = String::from_utf8(stderr).expect("utf8");
        assert!(
            rendered.ends_with("\r\n"),
            "in raw mode a status line must carriage-return so the next line starts at column 0: {rendered:?}",
        );
        assert!(
            !rendered.contains("\n\r") && rendered.matches('\n').count() == 1,
            "exactly one CRLF, no stray bare LF: {rendered:?}",
        );
    }

    #[test]
    fn lf_to_crlf_expands_every_newline_and_leaves_other_bytes() {
        assert_eq!(lf_to_crlf(b"a\nb\n"), b"a\r\nb\r\n");
        assert_eq!(lf_to_crlf(b"no newline"), b"no newline");
    }

    struct EmptyDecisions;
    impl
        lns_policy::decision_store::DecisionStore<
            lns_policy::host_bind_decisions::SecretDisposition,
        > for EmptyDecisions
    {
        fn load(&self) -> std::io::Result<lns_policy::host_bind_decisions::HostBindDecisionFile> {
            Ok(Default::default())
        }
        fn save(
            &self,
            _state: &lns_policy::host_bind_decisions::HostBindDecisionFile,
        ) -> std::io::Result<()> {
            panic!("a run with nothing to ask must record no decision")
        }
    }

    struct NoHostPathDecisions;
    impl
        lns_policy::decision_store::DecisionStore<lns_policy::host_path_decisions::HostPathDecision>
        for NoHostPathDecisions
    {
        fn load(&self) -> std::io::Result<lns_policy::host_path_decisions::HostPathDecisionFile> {
            Ok(Default::default())
        }
        fn save(
            &self,
            _state: &lns_policy::host_path_decisions::HostPathDecisionFile,
        ) -> std::io::Result<()> {
            panic!("a run with no hostPath fileset must record no decision")
        }
    }

    /// The stdin guard the questions need must not reach the attached session: tokio's stdin takes the same lock from a blocking thread, so a leaked guard leaves the run with a dead keyboard.
    #[test]
    fn the_questions_before_boot_leave_stdin_lockable_for_the_session() {
        let dir = tempfile::tempdir().expect("a temp dir for the bind source");
        std::fs::write(dir.path().join("Cargo.toml"), b"").expect("a file with no secret shape");
        let bind_specs = vec![lns_ipc::BindSpec {
            host_source: dir.path().to_string_lossy().into_owned(),
            target: "/work".into(),
            read_only: false,
            exclude: Vec::new(),
            optional: false,
        }];

        let (denied, binds) = ask_before_boot(&PreBootQuestions {
            pulled: None,
            origin: crate::run::host_path_consent::DocumentOrigin::OwnDirectory,
            filesets: &[],
            host_paths: &NoHostPathDecisions,
            bind_specs: &bind_specs,
            bind_decisions: &EmptyDecisions,
            assume_yes: false,
            interactive: false,
        })
        .expect("a local run with one clean bind asks nothing");
        assert!(denied.is_empty(), "no hostPath fileset can be denied");
        assert_eq!(binds.len(), 1, "the bind survives the questions");

        // What the stdin pump's blocking thread does once the run is attached.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            let _held = stdin.lock();
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("the questions must drop the stdin guard before the session reads the tty");
    }

    /// A resolve that locks stdin itself deadlocks every run that declares a bind, because the caller's guard is already held and stdin is not reentrant.
    #[test]
    fn resolving_host_binds_while_the_caller_holds_stdin_does_not_deadlock() {
        let dir = tempfile::tempdir().expect("a temp dir for the bind source");
        std::fs::write(dir.path().join("Cargo.toml"), b"").expect("a file with no secret shape");
        let specs = vec![lns_ipc::BindSpec {
            host_source: dir.path().to_string_lossy().into_owned(),
            target: "/work".into(),
            read_only: false,
            exclude: Vec::new(),
            optional: false,
        }];

        // Off the test thread, so a deadlock times out here instead of hanging the suite.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            let mut held = stdin.lock();
            let _ = tx.send(
                resolve_host_binds(&specs, false, &mut held, &EmptyDecisions)
                    .expect("a clean bind resolves"),
            );
        });

        let resolved = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("resolve_host_binds must return while the caller holds stdin, not block on it");
        assert_eq!(resolved.len(), 1, "the bind survives the resolve");
        assert!(
            resolved[0].dropped.is_empty(),
            "nothing here is secret-shaped"
        );
    }

    #[test]
    fn host_binds_interactive_is_suppressed_for_a_detached_run_even_with_a_tty() {
        assert!(
            host_binds_interactive(false, true),
            "attached + tty prompts"
        );
        assert!(
            !host_binds_interactive(true, true),
            "a detached run must never block on the secret prompt, even from a terminal"
        );
        assert!(
            !host_binds_interactive(false, false),
            "no terminal cannot prompt"
        );
    }

    #[test]
    fn needs_final_newline_truth_table() {
        assert!(
            !needs_final_newline(None),
            "no stdout means stderr status lines already left the cursor at column 0",
        );
        assert!(
            needs_final_newline(Some(b'o')),
            "non-newline last byte needs a newline"
        );
        assert!(
            !needs_final_newline(Some(b'\n')),
            "newline-terminated output does not need another",
        );
    }

    #[tokio::test]
    async fn drive_attached_session_appends_trailing_newline_when_workload_omits_one() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let out = lns_ipc::encode_wire_frame(&WireFrame::Stdout(b"hello".to_vec()))
                .expect("encode stdout");
            server.write_all(&out).await.expect("write stdout");
            let exit = encode_frame(&Response::RunExit { code: 0 }).expect("encode exit");
            server.write_all(&exit).await.expect("write exit");
        });
        let mut captured = Vec::<u8>::new();
        let mut status = Vec::<u8>::new();
        let code = drive_attached_session_with_writers(
            client,
            None,
            "1".to_string(),
            false,
            true,
            Vec::new(),
            DetachBehaviour::SignalAndDrain,
            &mut captured,
            &mut status,
            false,
            StdinForwarding::ToRun,
        )
        .await
        .expect("drive");
        assert_eq!(code, 0);
        assert_eq!(
            captured, b"hello\n",
            "expected exactly one appended newline"
        );
    }

    #[tokio::test]
    async fn exit_code_after_detach_leave_running_returns_zero_without_draining() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let exit = encode_frame(&Response::RunExit { code: 9 }).expect("encode exit");
        server.write_all(&exit).await.expect("write exit");

        let mut captured = Vec::<u8>::new();
        let mut status = Vec::<u8>::new();
        let mut last = None;
        let code = exit_code_after_detach(
            DetachBehaviour::LeaveRunning,
            false,
            &mut client,
            &mut captured,
            &mut status,
            &mut last,
            false,
        )
        .await;

        assert_eq!(code, 0, "detaching returns 0 at once");
        assert!(
            captured.is_empty(),
            "LeaveRunning must leave the run running, not drain its output"
        );
        let bytes = read_frame_bytes_async(&mut client)
            .await
            .expect("the workload's frame is still pending — it was not drained");
        assert!(matches!(
            decode_wire_frame_from_bytes(&bytes).expect("decode"),
            WireFrame::Json(Response::RunExit { code: 9 })
        ));
    }

    #[tokio::test]
    async fn exit_code_after_detach_detach_run_returns_zero_without_draining() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let exit = encode_frame(&Response::RunExit { code: 9 }).expect("encode exit");
        server.write_all(&exit).await.expect("write exit");

        let mut captured = Vec::<u8>::new();
        let mut status = Vec::<u8>::new();
        let mut last = None;
        let code = exit_code_after_detach(
            DetachBehaviour::DetachRun,
            false,
            &mut client,
            &mut captured,
            &mut status,
            &mut last,
            false,
        )
        .await;

        assert_eq!(code, 0, "detaching a run returns 0 at once");
        assert!(
            captured.is_empty(),
            "DetachRun must leave the run running, not drain its output"
        );
    }

    #[tokio::test]
    async fn exit_code_after_detach_signal_and_drain_adopts_the_drained_exit_code() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let exit = encode_frame(&Response::RunExit { code: 7 }).expect("encode exit");
        server.write_all(&exit).await.expect("write exit");

        let mut captured = Vec::<u8>::new();
        let mut status = Vec::<u8>::new();
        let mut last = None;
        let code = exit_code_after_detach(
            DetachBehaviour::SignalAndDrain,
            true,
            &mut client,
            &mut captured,
            &mut status,
            &mut last,
            false,
        )
        .await;

        assert_eq!(
            code, 7,
            "SignalAndDrain drains through to the real exit code"
        );
    }

    #[tokio::test]
    async fn drive_attached_session_does_not_double_newline_when_workload_ends_with_one() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let out = lns_ipc::encode_wire_frame(&WireFrame::Stdout(b"hello\n".to_vec()))
                .expect("encode stdout");
            server.write_all(&out).await.expect("write stdout");
            let exit = encode_frame(&Response::RunExit { code: 0 }).expect("encode exit");
            server.write_all(&exit).await.expect("write exit");
        });
        let mut captured = Vec::<u8>::new();
        let mut status = Vec::<u8>::new();
        let code = drive_attached_session_with_writers(
            client,
            None,
            "1".to_string(),
            false,
            true,
            Vec::new(),
            DetachBehaviour::SignalAndDrain,
            &mut captured,
            &mut status,
            false,
            StdinForwarding::ToRun,
        )
        .await
        .expect("drive");
        assert_eq!(code, 0);
        assert_eq!(captured, b"hello\n", "must not add a second newline");
    }

    #[tokio::test]
    async fn drive_attached_session_relays_piped_stdout_byte_for_byte_without_appending() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let out = lns_ipc::encode_wire_frame(&WireFrame::Stdout(b"hello".to_vec()))
                .expect("encode stdout");
            server.write_all(&out).await.expect("write stdout");
            let exit = encode_frame(&Response::RunExit { code: 0 }).expect("encode exit");
            server.write_all(&exit).await.expect("write exit");
        });
        let mut captured = Vec::<u8>::new();
        let mut status = Vec::<u8>::new();
        let code = drive_attached_session_with_writers(
            client,
            None,
            "1".to_string(),
            false,
            false,
            Vec::new(),
            DetachBehaviour::SignalAndDrain,
            &mut captured,
            &mut status,
            false,
            StdinForwarding::ToRun,
        )
        .await
        .expect("drive");
        assert_eq!(code, 0);
        assert_eq!(
            captured, b"hello",
            "a redirected/piped stdout must receive the workload's exact bytes",
        );
    }

    #[tokio::test]
    async fn drive_attached_session_renders_post_session_runlog_in_check_form() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let frame = encode_frame(&run_log("Finished", "in 2.53s")).expect("encode finished");
            server.write_all(&frame).await.expect("write finished");
            let exit = encode_frame(&Response::RunExit { code: 0 }).expect("encode exit");
            server.write_all(&exit).await.expect("write exit");
        });
        let mut captured = Vec::<u8>::new();
        let mut status = Vec::<u8>::new();
        let code = drive_attached_session_with_writers(
            client,
            None,
            "1".to_string(),
            false,
            true,
            Vec::new(),
            DetachBehaviour::SignalAndDrain,
            &mut captured,
            &mut status,
            false,
            StdinForwarding::ToRun,
        )
        .await
        .expect("drive");
        assert_eq!(code, 0);
        let s = String::from_utf8(status).unwrap();
        assert!(
            s.contains("✓ finished in 2.53s"),
            "missing check form: {s:?}"
        );
        assert!(
            !s.contains("     Finished"),
            "right-aligned padding leaked: {s:?}"
        );
    }

    #[test]
    fn a_session_opened_without_stdin_hands_the_run_nothing_it_typed() {
        for action in [
            FeedAction::Flush(b"echo pwned\n".to_vec()),
            FeedAction::FlushAndForward(b"echo pwned".to_vec(), b'\n'),
            FeedAction::Hold,
        ] {
            let mut pending = b"already typed".to_vec();
            let (requests, control) = plan_feed(
                action,
                "1",
                &mut pending,
                DetachBehaviour::SignalAndDrain,
                StdinForwarding::Withheld,
            );
            assert!(
                requests.is_empty(),
                "a non-interactive exec must not feed the run's own shell: {requests:?}"
            );
            assert!(matches!(control, PumpControl::Continue));
        }
    }

    #[test]
    fn a_session_opened_without_stdin_still_honours_the_detach_chord() {
        let mut pending = b"already typed".to_vec();
        let (requests, control) = plan_feed(
            FeedAction::Trigger,
            "9",
            &mut pending,
            DetachBehaviour::SignalAndDrain,
            StdinForwarding::Withheld,
        );
        assert_eq!(
            requests,
            vec![Request::SessionSignal {
                target: primary_target("9".to_string()),
                signal: SignalKind::Hup
            }],
            "the chord must still detach, and must not flush the withheld bytes on its way out"
        );
        assert!(matches!(control, PumpControl::Detach));
    }

    #[test]
    fn plan_feed_forward_accumulates_without_sending() {
        let mut pending = Vec::new();
        let (requests, control) = plan_feed(
            FeedAction::Forward(b'a'),
            "1",
            &mut pending,
            DetachBehaviour::SignalAndDrain,
            StdinForwarding::ToRun,
        );
        assert!(requests.is_empty());
        assert!(matches!(control, PumpControl::Continue));
        assert_eq!(pending, vec![b'a']);
    }

    #[test]
    fn plan_feed_hold_flushes_accumulated_bytes() {
        let mut pending = vec![b'x', b'y'];
        let (requests, _) = plan_feed(
            FeedAction::Hold,
            "7",
            &mut pending,
            DetachBehaviour::SignalAndDrain,
            StdinForwarding::ToRun,
        );
        assert_eq!(
            requests,
            vec![Request::SessionStdin {
                target: primary_target("7".to_string()),
                bytes: vec![b'x', b'y']
            }]
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn plan_feed_hold_with_nothing_pending_sends_nothing() {
        let mut pending = Vec::new();
        let (requests, _) = plan_feed(
            FeedAction::Hold,
            "7",
            &mut pending,
            DetachBehaviour::SignalAndDrain,
            StdinForwarding::ToRun,
        );
        assert!(requests.is_empty());
    }

    #[test]
    fn plan_feed_flush_sends_pending_before_the_held_bytes() {
        let mut pending = vec![b'p'];
        let (requests, _) = plan_feed(
            FeedAction::Flush(vec![b'h']),
            "3",
            &mut pending,
            DetachBehaviour::SignalAndDrain,
            StdinForwarding::ToRun,
        );
        assert_eq!(
            requests,
            vec![
                Request::SessionStdin {
                    target: primary_target("3".to_string()),
                    bytes: vec![b'p']
                },
                Request::SessionStdin {
                    target: primary_target("3".to_string()),
                    bytes: vec![b'h']
                },
            ]
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn plan_feed_flush_and_forward_appends_the_current_byte() {
        let mut pending = Vec::new();
        let (requests, _) = plan_feed(
            FeedAction::FlushAndForward(vec![b'h'], b'c'),
            "3",
            &mut pending,
            DetachBehaviour::SignalAndDrain,
            StdinForwarding::ToRun,
        );
        assert_eq!(
            requests,
            vec![Request::SessionStdin {
                target: primary_target("3".to_string()),
                bytes: vec![b'h', b'c']
            }]
        );
    }

    #[test]
    fn plan_feed_trigger_signals_hup_then_detaches_for_run_and_exec() {
        let mut pending = vec![b'p'];
        let (requests, control) = plan_feed(
            FeedAction::Trigger,
            "9",
            &mut pending,
            DetachBehaviour::SignalAndDrain,
            StdinForwarding::ToRun,
        );
        assert_eq!(
            requests,
            vec![
                Request::SessionStdin {
                    target: primary_target("9".to_string()),
                    bytes: vec![b'p']
                },
                Request::SessionSignal {
                    target: primary_target("9".to_string()),
                    signal: SignalKind::Hup
                },
            ]
        );
        assert!(matches!(control, PumpControl::Detach));
        assert!(pending.is_empty());
    }

    #[test]
    fn plan_feed_trigger_leaves_the_run_running_without_signalling_for_attach() {
        let mut pending = vec![b'p'];
        let (requests, control) = plan_feed(
            FeedAction::Trigger,
            "9",
            &mut pending,
            DetachBehaviour::LeaveRunning,
            StdinForwarding::ToRun,
        );
        assert_eq!(
            requests,
            vec![Request::SessionStdin {
                target: primary_target("9".to_string()),
                bytes: vec![b'p']
            }],
            "detaching flushes pending input but never signals the workload"
        );
        assert!(matches!(control, PumpControl::Detach));
        assert!(pending.is_empty());
    }

    #[test]
    fn plan_feed_trigger_sends_run_detach_without_signalling_for_run() {
        let mut pending = vec![b'p'];
        let (requests, control) = plan_feed(
            FeedAction::Trigger,
            "9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a",
            &mut pending,
            DetachBehaviour::DetachRun,
            StdinForwarding::ToRun,
        );
        assert_eq!(
            requests,
            vec![
                Request::SessionStdin {
                    target: primary_target("9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a".to_string()),
                    bytes: vec![b'p']
                },
                Request::SessionDetach {
                    target: primary_target("9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a".to_string()),
                },
            ],
            "detaching a run flushes pending input then asks the service to keep it running — no SIGHUP",
        );
        assert!(matches!(control, PumpControl::Detach));
        assert!(pending.is_empty());
    }

    #[test]
    fn plan_feed_close_session_targets_only_the_exec_session() {
        let target = lns_ipc::SessionTarget::Exec {
            run_id: "run-7".to_string(),
            session_id: "exec-2".to_string(),
        };
        let mut pending = Vec::new();

        let (requests, control) = plan_feed_target(
            FeedAction::Trigger,
            &target,
            &mut pending,
            DetachBehaviour::CloseSession,
            StdinForwarding::ToRun,
        );

        assert_eq!(requests, vec![Request::SessionDetach { target }]);
        assert!(matches!(control, PumpControl::Detach));
    }

    #[tokio::test(start_paused = true)]
    async fn write_and_await_ack_blocks_until_the_service_responds() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let detach = Request::SessionDetach {
            target: primary_target("7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a".to_string()),
        };
        let mut ack = std::pin::pin!(write_and_await_ack(&mut client, &detach));

        assert!(
            timeout(Duration::from_millis(50), &mut ack).await.is_err(),
            "the detach round-trip must not complete before the service acknowledges — \
             a fire-and-forget RunDetach would let the CLI close the run stream first and \
             reintroduce terminate-on-detach",
        );

        let req = read_frame_bytes_async(&mut server)
            .await
            .expect("server reads the request");
        let decoded: Request = decode_frame(&mut &req[..]).expect("decode request");
        assert_eq!(
            decoded,
            Request::SessionDetach {
                target: primary_target("7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a".to_string()),
            }
        );

        let frame = encode_frame(&Response::DetachAccepted).expect("encode ack");
        server.write_all(&frame).await.expect("server writes ack");

        ack.await
            .expect("the round-trip completes once the service acknowledges");
    }

    #[test]
    fn render_status_line_quiet_suppresses_info() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Info,
            Some("Resolved"),
            "ubuntu:latest",
            &mut buf,
            true,
        )
        .unwrap();
        assert!(buf.is_empty(), "quiet must suppress Info lines");
    }

    #[test]
    fn render_status_line_quiet_still_emits_warn() {
        let mut buf = Vec::<u8>::new();
        render_status_line(LogLevel::Warn, Some("SlowPull"), "retrying", &mut buf, true).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains("⚠ slowpull retrying"),
            "quiet must still emit Warn: {s:?}"
        );
    }

    #[test]
    fn render_status_line_quiet_still_emits_error() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Error,
            Some("Resolve"),
            "not found",
            &mut buf,
            true,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains("✗ resolve not found"),
            "quiet must still emit Error: {s:?}"
        );
    }

    #[test]
    fn render_started_run_quiet_suppresses_output() {
        let mut buf = Vec::<u8>::new();
        render_started_run("07eeeeee000000000000000000000000", &mut buf, true).unwrap();
        assert!(buf.is_empty(), "quiet must suppress the started-run line");
    }

    #[tokio::test]
    async fn drive_pre_phase_quiet_suppresses_info_but_detects_session_ready() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(&mut server, run_log("Resolved", "ubuntu:latest")).await;
            write_response(&mut server, run_log("Pulled", "7 layers")).await;
            write_response(&mut server, run_log("Booted", "microVM")).await;
            write_response(&mut server, run_log("SessionReady", "")).await;
        });
        let mut buf = Vec::<u8>::new();
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), true)
            .await
            .unwrap();
        assert_eq!(outcome, PrePhaseOutcome::SessionReady);
        assert!(
            buf.is_empty(),
            "quiet must suppress all Info status lines: {buf:?}"
        );
    }
}
