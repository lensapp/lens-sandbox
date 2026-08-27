use std::path::PathBuf;

use clap::FromArgMatches;
use lns_ipc::{Request, Response};

use super::ConnectorService;
use crate::command::{RunCtx, RunFuture};
use crate::local_future::LocalBoxFuture;

pub fn run<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::ConnectorArgs::from_arg_matches(matches)?;
        let cwd = ctx.cwd()?;
        crate::service::require_running().await?;
        let svc = RealConnectorService::new(crate::service::socket_path()?);
        let mut out = ctx.out;
        let mut terminal = crate::terminal::RealTerminal::open();
        crate::connector::run(
            &args.command,
            &svc,
            &mut terminal,
            &cwd,
            &mut out,
            &mut std::io::stderr(),
        )
        .await
    })
}

pub struct RealConnectorService {
    socket: PathBuf,
}

impl RealConnectorService {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
}

impl ConnectorService for RealConnectorService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>> {
        Box::pin(async move { crate::service::real::send_request(&self.socket, &req).await })
    }
}
