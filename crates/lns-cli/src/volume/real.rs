use std::path::PathBuf;

use clap::FromArgMatches;
use lns_ipc::{Request, Response};

use super::VolumeService;
use crate::command::{RunCtx, RunFuture};
use crate::connector::LocalBoxFuture;

pub fn run<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::VolumeArgs::from_arg_matches(matches)?;
        crate::service::require_running().await?;
        let svc = RealVolumeService::new(crate::service::socket_path()?);
        let mut out = ctx.out;
        crate::volume::run(&args.command, &svc, ctx.input, &mut out).await
    })
}

pub struct RealVolumeService {
    socket: PathBuf,
}

impl RealVolumeService {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
}

impl VolumeService for RealVolumeService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>> {
        Box::pin(async move { crate::service::real::send_request(&self.socket, &req).await })
    }
}
