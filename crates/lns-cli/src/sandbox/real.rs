use std::io::IsTerminal;
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
        let args = super::SandboxArgs::from_arg_matches(matches)?;
        if super::author::is_offline(&args.command) {
            return run_author(&args.command, ctx);
        }
        if let super::SandboxCommand::Push(push_args) = &args.command {
            let reference = push_args.reference.clone();
            return push_local(&reference, ctx.cwd()?).await;
        }
        crate::service::require_running().await;
        dispatch(args).await
    })
}

pub fn run_init<'a>(_matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move { run_author(&super::SandboxCommand::Init, ctx) })
}

pub fn run_ps<'a>(_matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move { dispatch_command(super::SandboxCommand::Ps).await })
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

pub fn run_inspect<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxInspectArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Inspect(args)).await
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
        push_local(&args.reference, ctx.cwd()?).await
    })
}

pub fn run_pull<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxPullArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Pull(args)).await
    })
}

pub fn run_tag<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxTagArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Tag(args)).await
    })
}

async fn push_local(reference: &str, cwd: PathBuf) -> Result<i32> {
    let doc = super::author::load_definition_json(&RealFs, &cwd)?;
    let mut out = std::io::stdout();
    super::distribute::push(&RealFs, &cwd, &RealProducer, &doc, reference, &mut out).await
}

struct RealProducer;

impl super::distribute::Producer for RealProducer {
    fn build_and_push<'a>(
        &'a self,
        doc: &'a [u8],
        reference: &'a str,
    ) -> crate::integration::LocalBoxFuture<'a, Result<String>> {
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
    ) -> crate::integration::LocalBoxFuture<'a, Result<()>> {
        Box::pin(async move { crate::build::push::push_artifact(built, reference).await })
    }
}

fn run_author(command: &super::SandboxCommand, ctx: RunCtx<'_>) -> Result<i32> {
    let cwd = ctx.cwd()?;
    let mut out = std::io::stdout();
    match command {
        super::SandboxCommand::Init => super::author::init(&RealFs, &cwd, &mut out),
        super::SandboxCommand::Validate => super::author::validate(&RealFs, &cwd, &mut out),
        super::SandboxCommand::Show => super::author::show(&RealFs, &cwd, &mut out),
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
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
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
            entries.push(super::author::DirEntry {
                name,
                dir: entry.file_type()?.is_dir(),
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
