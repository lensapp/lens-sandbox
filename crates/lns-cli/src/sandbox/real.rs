use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::FromArgMatches;
use lns_ipc::{Request, Response, encode_frame};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use super::{SandboxService, TermInfo, run_with_writers};
use crate::command::{RunCtx, RunFuture};
use crate::service::client::BoxFuture;

pub fn run<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let mut args = super::SandboxArgs::from_arg_matches(matches)?;
        qualify_references(&mut args.command)?;
        if let super::SandboxCommand::Run(run_args) = args.command {
            return crate::service::launch_run(*run_args, ctx.debug).await;
        }
        if super::author::is_offline(&args.command) {
            return run_author(&args.command, ctx);
        }
        if let super::SandboxCommand::Push(push_args) = &args.command {
            let reference = push_args.reference.clone();
            let file = push_args.file.clone();
            return push_local(&reference, push_args.dry_run, file.as_deref(), ctx.cwd()?).await;
        }
        crate::service::require_running().await;
        dispatch(args).await
    })
}

fn configured_registry() -> Result<Option<String>> {
    let path = crate::config::default_config_path()?;
    Ok(crate::config::load_run_defaults(&path)?.registry)
}

fn qualify_references(command: &mut super::SandboxCommand) -> Result<()> {
    super::apply_registry_default(command, configured_registry()?.as_deref());
    Ok(())
}

fn qualified_reference(reference: &str) -> Result<String> {
    Ok(crate::config::resolve_default_registry(
        reference,
        configured_registry()?.as_deref(),
    ))
}

pub fn run_init<'a>(_matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move { run_author(&super::SandboxCommand::Init, ctx) })
}

pub fn run_ps<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::PsArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Ps(args)).await
    })
}

async fn dispatch_command(command: super::SandboxCommand) -> Result<i32> {
    crate::service::require_running().await;
    dispatch(super::SandboxArgs { command }).await
}

pub fn run_stop<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxStopArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Stop(args)).await
    })
}

pub fn run_kill<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = crate::cli::KillArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Kill(args)).await
    })
}

pub fn run_rm<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxRmArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Rm(args)).await
    })
}

pub fn run_inspect<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxInspectArgs::from_arg_matches(matches)?;
        let command = super::SandboxCommand::Inspect(args);
        if super::author::is_offline(&command) {
            return run_author(&command, ctx);
        }
        dispatch_command(command).await
    })
}

pub fn run_logs<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxLogsArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Logs(args)).await
    })
}

pub fn run_attach<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxAttachArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Attach(args)).await
    })
}

pub fn run_push<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxPushArgs::from_arg_matches(matches)?;
        push_local(
            &qualified_reference(&args.reference)?,
            args.dry_run,
            args.file.as_deref(),
            ctx.cwd()?,
        )
        .await
    })
}

pub fn run_pull<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let mut args = super::SandboxPullArgs::from_arg_matches(matches)?;
        args.reference = qualified_reference(&args.reference)?;
        dispatch_command(super::SandboxCommand::Pull(args)).await
    })
}

pub fn run_tag<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxTagArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Tag(args)).await
    })
}

async fn push_local(
    reference: &str,
    dry_run: bool,
    file: Option<&std::path::Path>,
    cwd: PathBuf,
) -> Result<i32> {
    let path = super::author::selected_definition_path(file, &cwd);
    let project_dir = path.parent().unwrap_or(&cwd).to_path_buf();
    let doc = super::author::load_definition_json_at(&RealFs, &path)?;
    let mut out = std::io::stdout();
    if dry_run {
        return super::distribute::push_dry_run(&RealFs, &project_dir, &doc, reference, &mut out);
    }
    super::distribute::push(
        &RealFs,
        &project_dir,
        &RealProducer,
        &RealToolResolver,
        &doc,
        reference,
        &mut out,
    )
    .await
}

struct RealToolResolver;

const TOOL_INDEX_URL: &str = "https://mise-versions.jdx.dev";
/// A push must fail with a diagnostic rather than hang against a blackholing proxy, and the index body is a version list, not a download.
const TOOL_INDEX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const MAX_TOOL_INDEX_BYTES: usize = 8 * 1024 * 1024;

impl super::distribute::ToolResolver for RealToolResolver {
    fn resolve<'a>(
        &'a self,
        tool: &'a lns_artifact::tools::ToolRef,
    ) -> crate::connector::LocalBoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let base =
                std::env::var("LNS_TOOL_INDEX_URL").unwrap_or_else(|_| TOOL_INDEX_URL.to_string());
            let url = format!("{}/{}", base.trim_end_matches('/'), tool.name);
            let response = reqwest::Client::builder()
                .timeout(TOOL_INDEX_TIMEOUT)
                .build()
                .context("building the tool version index client")?
                .get(&url)
                .send()
                .await
                .with_context(|| format!("querying the tool version index at {url}"))?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                anyhow::bail!(
                    "tool {:?} is unknown to the version index ({url}); check the name against mise's registry",
                    tool.name
                );
            }
            let body = response
                .error_for_status()
                .with_context(|| format!("tool version index at {url}"))?
                .text()
                .await
                .with_context(|| format!("reading the version index at {url}"))?;
            anyhow::ensure!(
                body.len() <= MAX_TOOL_INDEX_BYTES,
                "the version index at {url} returned {} bytes, over the {MAX_TOOL_INDEX_BYTES}-byte limit",
                body.len()
            );
            lns_artifact::tools::resolve_from_index(&tool.name, &tool.version, &body)
        })
    }
}

struct RealProducer;

impl super::distribute::Producer for RealProducer {
    fn build_and_push<'a>(
        &'a self,
        doc: &'a [u8],
        reference: &'a str,
    ) -> crate::connector::LocalBoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let built = lns_artifact::build::build_artifact(doc)?;
            crate::build::push::push_artifact(&built, reference).await?;
            Ok(built.manifest_digest)
        })
    }

    fn push_prebuilt<'a>(
        &'a self,
        built: &'a lns_artifact::build::BuiltArtifact,
        reference: &'a str,
    ) -> crate::connector::LocalBoxFuture<'a, Result<()>> {
        Box::pin(async move { crate::build::push::push_artifact(built, reference).await })
    }
}

fn run_author(command: &super::SandboxCommand, ctx: RunCtx<'_>) -> Result<i32> {
    let cwd = ctx.cwd()?;
    let mut out = std::io::stdout();
    match command {
        super::SandboxCommand::Init => super::author::init(&RealFs, &cwd, &mut out),
        super::SandboxCommand::Validate(args) => {
            super::author::validate(&RealFs, &cwd, args.file.as_deref(), &mut out)
        }
        super::SandboxCommand::Inspect(args) => super::author::inspect_local(
            &RealFs,
            &cwd,
            args.run.as_deref(),
            args.file.as_deref(),
            &mut out,
        ),
        _ => unreachable!("run_author is only called for offline author verbs"),
    }
}

pub(crate) struct RealFs;

impl super::author::Fs for RealFs {
    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }
    fn write(&self, path: &Path, contents: &str) -> std::io::Result<()> {
        std::fs::write(path, contents)
    }
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn read_limited(&self, path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    fn dir_entries(&self, dir: &Path) -> std::io::Result<Vec<super::author::DirEntry>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().into_string().map_err(|name| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("non-utf8 file name {name:?}"),
                )
            })?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("symlink {name} — filesets carry only regular files"),
                ));
            }
            use std::os::unix::fs::PermissionsExt;
            entries.push(super::author::DirEntry {
                name,
                dir: file_type.is_dir(),
                mode: entry.metadata()?.permissions().mode() & 0o777,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }
}

pub struct RealSandboxService {
    socket: PathBuf,
}

impl SandboxService for RealSandboxService {
    type Stream = UnixStream;

    fn one_shot(&self, request: Request) -> BoxFuture<'_, Result<Response>> {
        Box::pin(async move {
            crate::service::real::send_request(&self.socket, &request)
                .await
                .ok_or_else(|| anyhow::anyhow!("no response from lns-service (is it running?)"))
        })
    }

    fn open_stream(&self, request: Request) -> BoxFuture<'_, Result<UnixStream>> {
        Box::pin(async move {
            let mut stream = UnixStream::connect(&self.socket)
                .await
                .with_context(|| format!("connecting to {}", self.socket.display()))?;
            let frame = encode_frame(&request).context("encoding stream request")?;
            stream
                .write_all(&frame)
                .await
                .context("writing stream request")?;
            Ok(stream)
        })
    }

    fn aux_socket(&self) -> Option<PathBuf> {
        Some(self.socket.clone())
    }

    fn load_policy(&self, path: &str) -> Option<serde_json::Value> {
        let path = Path::new(path);
        if !path.exists() {
            return None;
        }
        let policy = lns_policy::Policy::load_or_default(path).ok()?;
        serde_json::to_value(&policy).ok()
    }
}

pub async fn dispatch(args: super::SandboxArgs) -> Result<i32> {
    let command = match args.command {
        super::SandboxCommand::Exec(exec_args) => {
            return crate::service::exec_image(exec_args).await;
        }
        other => other,
    };
    let svc = RealSandboxService {
        socket: crate::service::socket_path()?,
    };
    let term = TermInfo {
        stdin_is_tty: crate::raw_mode::stdin_is_tty(),
        stdout_is_terminal: std::io::stdout().is_terminal(),
    };
    let mut out = std::io::stdout();
    let mut stdout = tokio::io::stdout();
    let mut stderr = tokio::io::stderr();
    run_with_writers(&command, &svc, term, &mut out, &mut stdout, &mut stderr).await
}
