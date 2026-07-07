use std::io;
use std::path::Path;

use anyhow::{Context, Result};
use clap::FromArgMatches;

use super::collect::{self, DirTree, Entry};
use super::{BuildArgs, push, report, resolve};
use crate::command::{RunCtx, RunFuture};

struct RealDirTree;

impl DirTree for RealDirTree {
    fn read_dir(&self, dir: &Path) -> io::Result<Vec<Entry>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            out.push(if file_type.is_dir() {
                Entry::Dir(path)
            } else if file_type.is_file() {
                Entry::File(path)
            } else {
                Entry::Other
            });
        }
        Ok(out)
    }

    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }
}

pub fn run_command<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = BuildArgs::from_arg_matches(matches)?;
        let path = ctx.cwd()?.join(&args.path);
        let mut out = ctx.out;
        let tag = args.tag.as_deref();

        if path.is_dir() {
            let Some(mount) = args.mount.as_deref() else {
                writeln!(out, "✖ a directory PATH requires --mount <path>")?;
                return Ok(1);
            };
            let name = collect::fileset_name(&args.path);
            let entries = collect::collect_dir(&RealDirTree, &path)?;
            let Some(built) =
                report::build_fileset_and_report(&name, mount, &entries, tag, &mut out)?
            else {
                return Ok(1);
            };
            if let Some(reference) = tag {
                cache_built(reference, &built)?;
                writeln!(out, "  cached for push: {reference}")?;
            }
            if !args.push {
                return Ok(0);
            }
            let target = report::push_target(tag)?;
            push::push_artifact(&built, target).await?;
            writeln!(out, "✔ pushed {target}@{}", built.manifest_digest)?;
            return Ok(0);
        }

        let raw =
            std::fs::read(&path).with_context(|| format!("reading manifest {}", path.display()))?;

        if args.check {
            return report::check_and_report(&raw, &args.path, &mut out);
        }

        let raw = if args.push || args.pin {
            match resolve::resolve_and_pin(&raw).await {
                Ok(pinned) => pinned,
                Err(e) => {
                    writeln!(out, "✖ {e:#}")?;
                    return Ok(1);
                }
            }
        } else {
            raw
        };

        let Some(built) = report::build_and_report(&raw, tag, &mut out)? else {
            return Ok(1);
        };
        if let Some(reference) = tag {
            cache_built(reference, &built)?;
            writeln!(out, "  cached for push: {reference}")?;
        }
        if !args.push {
            return Ok(0);
        }
        let target = report::push_target(tag)?;
        push::push_artifact(&built, target).await?;
        writeln!(out, "✔ pushed {target}@{}", built.manifest_digest)?;
        Ok(0)
    })
}

fn cache_built(reference: &str, built: &lns_artifact::build::BuiltArtifact) -> Result<()> {
    let root = lns_ipc::build_cache_root().context("resolving the build cache root")?;
    std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
    for file in super::cache::plan_writes(&root, reference, built)? {
        std::fs::write(&file.path, &file.bytes)
            .with_context(|| format!("writing {}", file.path.display()))?;
    }
    Ok(())
}
