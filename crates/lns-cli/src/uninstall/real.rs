use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::FromArgMatches;
use lns_ipc::{Request, Response};

use super::{Deps, Fs, LoginAgent, PurgeSources, UninstallArgs, UninstallPlan, UninstallService};
use crate::command::{RunCtx, RunFuture};
use crate::connector::LocalBoxFuture;
use crate::service::real::RealServiceClient;
use crate::service::{DisableOutcome, disable_login_agent};

pub fn run_command<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = UninstallArgs::from_arg_matches(matches)?;
        run(args, ctx).await
    })
}

async fn run(args: UninstallArgs, ctx: RunCtx<'_>) -> Result<i32> {
    let plan = build_plan(args.purge).await?;
    let socket = crate::service::socket_path()?;
    let svc = RealUninstallService {
        socket: socket.clone(),
    };
    let client = RealServiceClient::new(socket, crate::service::find_service_binary());
    let agent = RealLoginAgent;
    let fs = RealFs;
    let deps = Deps {
        svc: &svc,
        client: &client,
        agent: &agent,
        fs: &fs,
    };
    let mut terminal = crate::terminal::RealTerminal::open();
    let mut out = ctx.out;
    super::run_with(
        &args,
        &plan,
        &deps,
        &mut terminal,
        &mut out,
        &mut tokio::io::stderr(),
    )
    .await
}

async fn build_plan(purge: bool) -> Result<UninstallPlan> {
    let lns = std::env::current_exe().context("resolving the current `lns` executable path")?;
    let lns = tokio::fs::canonicalize(&lns)
        .await
        .with_context(|| format!("canonicalizing {}", lns.display()))?;
    let mut binaries = Vec::new();
    if let Some(parent) = lns.parent() {
        let service = parent.join("lns-service");
        if service.exists() {
            binaries.push(service);
        }
    }
    binaries.push(lns);
    let (purge_dirs, purge_files) = if purge {
        purge_targets_from_env()?
    } else {
        (Vec::new(), Vec::new())
    };
    Ok(UninstallPlan {
        binaries,
        purge_dirs,
        purge_files,
    })
}

fn purge_targets_from_env() -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    super::purge_targets(PurgeSources {
        lns_home: lns_ipc::lns_home().context("resolving the lns home directory")?,
        home: dirs::home_dir(),
        socket: crate::service::socket_path()?,
        socket_overridden: std::env::var_os("LNS_SOCKET_PATH").is_some(),
    })
}

struct RealUninstallService {
    socket: PathBuf,
}

impl UninstallService for RealUninstallService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>> {
        Box::pin(async move { crate::service::real::send_request(&self.socket, &req).await })
    }
}

struct RealLoginAgent;

impl LoginAgent for RealLoginAgent {
    fn disable(&self) -> LocalBoxFuture<'_, DisableOutcome> {
        Box::pin(async move { disable_login_agent().await })
    }
}

struct RealFs;

impl Fs for RealFs {
    fn remove_file(&self, path: &Path) -> LocalBoxFuture<'_, std::io::Result<()>> {
        let path = path.to_path_buf();
        Box::pin(async move { tokio::fs::remove_file(&path).await })
    }
    fn remove_dir_all(&self, path: &Path) -> LocalBoxFuture<'_, std::io::Result<()>> {
        let path = path.to_path_buf();
        Box::pin(async move { tokio::fs::remove_dir_all(&path).await })
    }
}
