//! The two shortcuts' production wiring: open the socket, ask both namespaces, then dispatch into whichever one owns the word.

use clap::FromArgMatches;

use super::{Owner, names_a_document, which};
use crate::command::{RunCtx, RunFuture};

pub(super) fn run_rm<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::ShortcutRmArgs::from_arg_matches(matches)?;
        crate::service::require_running().await?;
        let svc = crate::service::real::RealSandboxService::new(crate::service::socket_path()?);
        let default_registry = crate::artifact::real::configured_registry()?;
        let owner = super::rm_route(
            which(&svc, "rm", &args.reference, default_registry.as_deref()).await?,
            args.force,
        )?;
        match owner {
            Owner::Artifact => {
                let mut command = crate::artifact::ArtifactCommand::Rm(crate::artifact::RmArgs {
                    reference: args.reference,
                });
                crate::artifact::apply_registry_default(&mut command, default_registry.as_deref());
                crate::artifact::real::dispatch(command).await
            }
            _ => {
                crate::sandbox::real::dispatch(crate::sandbox::SandboxArgs {
                    command: crate::sandbox::SandboxCommand::Rm(crate::sandbox::SandboxRmArgs {
                        run: args.reference,
                        force: args.force,
                    }),
                })
                .await
            }
        }
    })
}

pub(super) fn run_inspect<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::ShortcutInspectArgs::from_arg_matches(matches)?;
        let format_given = args.format.is_some();
        let mixin_given = !args.artifact.mixins.is_empty();
        let refuse = |target| super::refusal(target, format_given, mixin_given);
        if args.artifact.file.is_some() || names_a_document(args.artifact.reference.as_deref()) {
            if let Some(message) = refuse(super::InspectTarget::Document) {
                return usage_error(&message);
            }
            return crate::artifact::real::run_inspect_offline(args.artifact, ctx);
        }
        let reference = args
            .artifact
            .reference
            .clone()
            .expect("a reference-less inspect is a local document, settled above");
        crate::service::require_running().await?;
        let svc = crate::service::real::RealSandboxService::new(crate::service::socket_path()?);
        let default_registry = crate::artifact::real::configured_registry()?;
        match which(&svc, "inspect", &reference, default_registry.as_deref()).await? {
            Owner::Artifact => {
                if let Some(message) = refuse(super::InspectTarget::Artifact) {
                    return usage_error(&message);
                }
                let mut inspect = args.artifact;
                if !inspect.mixins.is_empty() {
                    inspect.root_mixins(&ctx.cwd()?)?;
                }
                let mut command = crate::artifact::ArtifactCommand::Inspect(inspect);
                crate::artifact::apply_registry_default(&mut command, default_registry.as_deref());
                crate::artifact::real::dispatch(command).await
            }
            _ => {
                if let Some(message) = refuse(super::InspectTarget::Sandbox) {
                    return usage_error(&message);
                }
                crate::sandbox::real::dispatch(crate::sandbox::SandboxArgs {
                    command: crate::sandbox::SandboxCommand::Inspect(
                        crate::sandbox::SandboxInspectArgs {
                            run: reference,
                            output: crate::output::OutputArgs {
                                format: args.format.unwrap_or(crate::output::Format::Table),
                            },
                        },
                    ),
                })
                .await
            }
        }
    })
}

fn usage_error(message: &str) -> anyhow::Result<i32> {
    eprintln!("error: {message}");
    Ok(2)
}
