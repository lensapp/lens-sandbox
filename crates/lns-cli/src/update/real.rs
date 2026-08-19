use anyhow::{Context, Result};
use clap::FromArgMatches;

use crate::command::{RunCtx, RunFuture};
use crate::service::real::RealServiceClient;
use crate::update::UpdateArgs;

pub fn run_command<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = UpdateArgs::from_arg_matches(matches)?;
        run(args).await
    })
}

pub async fn run(args: UpdateArgs) -> Result<i32> {
    if args.dry_run {
        crate::update_check::real::run_dry_run()?;
        return Ok(0);
    }
    let lns_path = std::env::current_exe().context("resolving current `lns` executable path")?;
    let service = RealServiceClient::new(
        crate::service::socket_path()?,
        crate::service::find_service_binary(),
    );
    super::run_with(
        args,
        super::LNS_VERSION,
        super::DEFAULT_CDN_BASE,
        &crate::platform::detect(),
        &lns_path,
        &service,
    )
    .await
}
