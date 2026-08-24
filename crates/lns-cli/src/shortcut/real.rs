//! The two shortcuts' production wiring: open the socket, ask both namespaces, then dispatch into whichever one owns the word.

use clap::FromArgMatches;

use super::{Owner, names_a_document, which};
use crate::command::{RunCtx, RunFuture};

pub(super) fn run_rm<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = crate::artifact::RmArgs::from_arg_matches(matches)?;
        crate::service::require_running().await?;
        let svc = crate::service::real::RealSandboxService::new(crate::service::socket_path()?);
        match which(&svc, "rm", &args.reference).await? {
            Owner::Artifact => {
                crate::artifact::real::dispatch(
                    crate::artifact::ArtifactCommand::Rm(args),
                    ctx.input,
                )
                .await
            }
            _ => {
                crate::sandbox::real::dispatch(
                    crate::sandbox::SandboxArgs {
                        command: crate::sandbox::SandboxCommand::Rm(
                            crate::sandbox::SandboxRmArgs {
                                run: args.reference,
                                force: false,
                            },
                        ),
                    },
                    ctx.input,
                )
                .await
            }
        }
    })
}

pub(super) fn run_inspect<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = crate::artifact::InspectArgs::from_arg_matches(matches)?;
        if args.file.is_some() || names_a_document(args.reference.as_deref()) {
            return crate::artifact::real::run_inspect_offline(args, ctx);
        }
        let reference = args
            .reference
            .clone()
            .expect("a reference-less inspect is a local document, settled above");
        crate::service::require_running().await?;
        let svc = crate::service::real::RealSandboxService::new(crate::service::socket_path()?);
        match which(&svc, "inspect", &reference).await? {
            Owner::Artifact => {
                crate::artifact::real::dispatch(
                    crate::artifact::ArtifactCommand::Inspect(args),
                    ctx.input,
                )
                .await
            }
            _ => {
                crate::sandbox::real::dispatch(
                    crate::sandbox::SandboxArgs {
                        command: crate::sandbox::SandboxCommand::Inspect(
                            crate::sandbox::SandboxInspectArgs {
                                run: reference,
                                output: crate::output::OutputArgs {
                                    format: crate::output::Format::Table,
                                },
                            },
                        ),
                    },
                    ctx.input,
                )
                .await
            }
        }
    })
}
