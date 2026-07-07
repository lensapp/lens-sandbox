use crate::command::{CommandSpec, subcommand};

mod push;
mod report;
mod run;

#[derive(clap::Args)]
pub struct BuildArgs {
    #[arg(
        value_name = "PATH",
        help = "Path to an artifact manifest (YAML or JSON)."
    )]
    pub path: std::path::PathBuf,
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
