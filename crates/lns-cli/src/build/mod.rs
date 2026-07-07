use crate::command::{CommandSpec, subcommand};

pub(crate) mod cache;
mod collect;
pub(crate) mod push;
mod report;
mod resolve;
mod run;

#[derive(clap::Args)]
pub struct BuildArgs {
    #[arg(
        value_name = "PATH",
        help = "An artifact manifest (YAML or JSON), or a directory to package as a FileSet (requires --mount)."
    )]
    pub path: std::path::PathBuf,
    #[arg(
        long = "mount",
        value_name = "PATH",
        help = "Mount path for a directory packaged as a FileSet, e.g. /root/.some-agent/skills."
    )]
    pub mount: Option<String>,
    #[arg(
        short = 't',
        long = "tag",
        value_name = "REF",
        help = "Target registry ref for the built artifact (required with --push)."
    )]
    pub tag: Option<String>,
    #[arg(
        long,
        help = "Validate only: schema + cross-field guards + secret guard. Skips assembly and push."
    )]
    pub check: bool,
    #[arg(
        long,
        help = "Push the built artifact to its --tag ref, reusing the stored `lns login` credential (needs push scope)."
    )]
    pub push: bool,
    #[arg(
        long,
        help = "Resolve a bundle's floating component tags to their current digests and pin them (implied by --push; a plain build stays offline and refuses floating tags)."
    )]
    pub pin: bool,
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<BuildArgs>("build")
            .about("Validate, build, and push lens OCI artifacts from a local manifest."),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "build",
    augment,
    run: run::run_command,
    announces_update_check: true,
    owns_terminal: false,
};
