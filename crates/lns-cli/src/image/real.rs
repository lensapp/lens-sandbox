use std::path::PathBuf;

use clap::FromArgMatches;
use lns_ipc::{Request, Response};

use super::ImageService;
use crate::command::{RunCtx, RunFuture};
use crate::integration::LocalBoxFuture;

pub fn run<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::ImageArgs::from_arg_matches(matches)?;
        crate::service::require_running().await;
        let svc = RealImageService::new(crate::service::socket_path()?);
        let mut out = ctx.out;
        crate::image::run(&args.command, &svc, ctx.input, &mut out).await
    })
}

pub struct RealImageService {
    socket: PathBuf,
}

impl RealImageService {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
}

impl ImageService for RealImageService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>> {
        Box::pin(async move { crate::service::real::send_request(&self.socket, &req).await })
    }
}
