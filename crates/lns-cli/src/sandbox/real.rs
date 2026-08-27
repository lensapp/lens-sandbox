use std::io::IsTerminal;

use anyhow::Result;
use clap::FromArgMatches;

use super::{TermInfo, run_with_writers};
use crate::command::{RunCtx, RunFuture};
use crate::service::real::RealSandboxService;

pub fn run<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxArgs::from_arg_matches(matches)?;
        if let Some(refusal) = super::wrong_kind_refusal(&args.command) {
            eprintln!("error: {refusal}");
            return Ok(2);
        }
        match args.command {
            super::SandboxCommand::Run(run_args) => Ok(crate::service::as_pre_start_failure(
                crate::service::launch_run(*run_args, ctx.debug).await,
            )),
            super::SandboxCommand::Exec(exec_args) => {
                let started = async {
                    crate::service::require_running().await?;
                    crate::service::exec_image(exec_args).await
                };
                Ok(crate::service::as_pre_start_failure(started.await))
            }
            command => {
                crate::service::require_running().await?;
                dispatch(super::SandboxArgs { command }).await
            }
        }
    })
}

pub fn run_ps<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxLsArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Ls(args)).await
    })
}

async fn dispatch_command(command: super::SandboxCommand) -> Result<i32> {
    if let Some(refusal) = super::wrong_kind_refusal(&command) {
        eprintln!("error: {refusal}");
        return Ok(2);
    }
    crate::service::require_running().await?;
    dispatch(super::SandboxArgs { command }).await
}

pub fn run_start<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::SandboxStartArgs::from_arg_matches(matches)?;
        dispatch_command(super::SandboxCommand::Start(args)).await
    })
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

pub async fn dispatch(args: super::SandboxArgs) -> Result<i32> {
    let command = args.command;
    let svc = RealSandboxService::new(crate::service::socket_path()?);
    let term = TermInfo {
        stdin_is_tty: crate::raw_mode::stdin_is_tty(),
        stdout_is_terminal: std::io::stdout().is_terminal(),
    };
    let mut out = std::io::stdout();
    let mut stdout = tokio::io::stdout();
    let mut stderr = tokio::io::stderr();
    let mut terminal = crate::terminal::RealTerminal::open();
    run_with_writers(
        &command,
        &svc,
        term,
        &mut terminal,
        &mut out,
        &mut stdout,
        &mut stderr,
    )
    .await
}
