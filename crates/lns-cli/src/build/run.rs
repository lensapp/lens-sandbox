use anyhow::Context;
use clap::FromArgMatches;

use super::report;
use super::{BuildArgs, push};
use crate::command::{RunCtx, RunFuture};

pub fn run_command<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = BuildArgs::from_arg_matches(matches)?;
        let path = ctx.cwd()?.join(&args.path);
        let raw =
            std::fs::read(&path).with_context(|| format!("reading manifest {}", path.display()))?;
        let mut out = ctx.out;

        if args.check {
            return report::check_and_report(&raw, &args.path, &mut out);
        }

        let tag = args.tag.as_deref();
        let Some(built) = report::build_and_report(&raw, tag, &mut out)? else {
            return Ok(1);
        };
        if !args.push {
            return Ok(0);
        }
        let target = report::push_target(tag)?;
        push::push_artifact(&built, target).await?;
        writeln!(out, "✔ pushed {target}@{}", built.manifest_digest)?;
        Ok(0)
    })
}
