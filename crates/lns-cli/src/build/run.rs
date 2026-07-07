use std::path::Path;

use anyhow::{Context, Result};
use clap::FromArgMatches;
use lns_artifact::build::FileEntry;

use super::{BuildArgs, push, report, resolve};
use crate::command::{RunCtx, RunFuture};

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
            let name = fileset_name(&args.path);
            let entries = collect_dir(&path)?;
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

fn fileset_name(path: &Path) -> String {
    let raw = path.file_name().map(|n| n.to_string_lossy());
    report::sanitize_name(raw.as_deref().unwrap_or("fileset"))
}

fn collect_dir(root: &Path) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    walk_dir(root, root, &mut entries)?;
    Ok(entries)
}

fn walk_dir(root: &Path, dir: &Path, out: &mut Vec<FileEntry>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            walk_dir(root, &path, out)?;
        } else if file_type.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("walked path is under root")
                .to_string_lossy()
                .into_owned();
            let data =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            out.push(FileEntry { path: rel, data });
        }
    }
    Ok(())
}
