use anyhow::Context;
use clap::FromArgMatches;

use crate::build::cache;
use crate::build::push::push_artifact;
use crate::command::{CommandSpec, RunCtx, RunFuture, subcommand};

#[derive(clap::Args)]
pub struct PushArgs {
    #[arg(
        value_name = "REF",
        help = "A ref previously produced by `lns build -t <REF>`; its cached artifact is uploaded as-is."
    )]
    pub reference: String,
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<PushArgs>("push")
            .about("Push an artifact from the local build cache to its registry ref."),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "push",
    augment,
    run: run_command,
    announces_update_check: true,
    owns_terminal: false,
};

pub fn run_command<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = PushArgs::from_arg_matches(matches)?;
        let out = ctx.out;
        let root = lns_ipc::build_cache_root().context("resolving the build cache root")?;

        let record_bytes = match std::fs::read(cache::ref_record_path(&root, &args.reference)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                writeln!(
                    out,
                    "✖ nothing cached for {0}; run `lns build -t {0}` first",
                    args.reference
                )?;
                return Ok(1);
            }
            Err(e) => return Err(e).context("reading the build-cache ref record"),
        };
        let record: cache::RefRecord =
            serde_json::from_slice(&record_bytes).context("parsing the build-cache ref record")?;
        let manifest = std::fs::read(cache::blob_path(&root, &record.manifest_digest)?)
            .context("reading the cached manifest")?;
        let built = cache::reconstruct(&manifest, &record.manifest_digest, |digest| {
            std::fs::read(cache::blob_path(&root, digest)?)
                .with_context(|| format!("reading cached blob {digest}"))
        })?;

        push_artifact(&built, &args.reference).await?;
        writeln!(out, "✔ pushed {}@{}", args.reference, built.manifest_digest)?;
        Ok(0)
    })
}
