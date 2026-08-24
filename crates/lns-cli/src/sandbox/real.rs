use std::io::IsTerminal;

use anyhow::Result;
use clap::FromArgMatches;

use super::{TermInfo, run_with_writers};
use crate::command::{RunCtx, RunFuture};
use crate::service::real::RealSandboxService;

pub fn run<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxArgs::from_arg_matches(matches)?;
        if let super::SandboxCommand::Run(run_args) = args.command {
            return crate::service::launch_run(*run_args, ctx.debug).await;
        }
        crate::service::require_running().await?;
        dispatch(args, ctx.input).await
    })
}

pub fn run_ps<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxLsArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Ls(args), ctx.input).await
    })
}

async fn dispatch_command(
    command: super::SandboxCommand,
    input: &mut dyn std::io::BufRead,
) -> Result<i32> {
    crate::service::require_running().await?;
    dispatch(super::SandboxArgs { command }, input).await
}

pub fn run_start<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxStartArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Start(args), ctx.input).await
    })
}

pub fn run_stop<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxStopArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Stop(args), ctx.input).await
    })
}

pub fn run_kill<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = crate::cli::KillArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Kill(args), ctx.input).await
    })
}

pub fn run_rm<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxRmArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Rm(args), ctx.input).await
    })
}

pub fn run_logs<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxLogsArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Logs(args), ctx.input).await
    })
}

pub fn run_attach<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxAttachArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Attach(args), ctx.input).await
    })
}

// The caller already holds the process-wide stdin lock (run_from_matches), so this must borrow it — a second Stdin::lock on the same thread deadlocks every dispatched verb.
pub async fn dispatch(args: super::SandboxArgs, input: &mut dyn std::io::BufRead) -> Result<i32> {
    let command = match args.command {
        super::SandboxCommand::Exec(exec_args) => {
            return crate::service::exec_image(exec_args).await;
        }
        other => other,
    };
    let svc = RealSandboxService::new(crate::service::socket_path()?);
    let term = TermInfo {
        stdin_is_tty: crate::raw_mode::stdin_is_tty(),
        stdout_is_terminal: std::io::stdout().is_terminal(),
    };
    let mut out = std::io::stdout();
    let mut stdout = tokio::io::stdout();
    let mut stderr = tokio::io::stderr();
    let mut input = input;
    run_with_writers(
        &command,
        &svc,
        term,
        &mut input,
        &mut out,
        &mut stdout,
        &mut stderr,
    )
    .await
}
