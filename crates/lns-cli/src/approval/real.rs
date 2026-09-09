use std::path::PathBuf;

use clap::FromArgMatches;
use lns_ipc::{Request, Response};

use super::ApprovalService;
use crate::command::{RunCtx, RunFuture};
use crate::local_future::LocalBoxFuture;

pub fn run<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::ApprovalArgs::from_arg_matches(matches)?;
        crate::service::require_running().await?;
        let svc = RealApprovalService::new(crate::service::socket_path()?);
        let mut out = ctx.out;
        crate::approval::run(&args.command, &svc, &mut out).await
    })
}

pub struct RealApprovalService {
    socket: PathBuf,
}

impl RealApprovalService {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
}

impl ApprovalService for RealApprovalService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>> {
        Box::pin(async move { crate::service::real::send_request(&self.socket, &req).await })
    }
}
