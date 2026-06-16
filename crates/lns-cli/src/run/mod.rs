pub mod env_file;
pub mod progress;
pub mod summary;

use crate::command::{CommandSpec, subcommand};

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(subcommand::<crate::cli::RunArgs>("run").about("Run an OCI image in a microVM."))
}

pub const RUN_SPEC: CommandSpec = CommandSpec {
    name: "run",
    augment,
    run: crate::service::run_command,
    announces_update_check: true,
    owns_terminal: true,
};

pub fn augment_exec(app: clap::Command) -> clap::Command {
    app.subcommand(subcommand::<crate::cli::ExecArgs>("exec").hide(true))
}

pub const EXEC_SPEC: CommandSpec = CommandSpec {
    name: "exec",
    augment: augment_exec,
    run: crate::service::exec_command,
    announces_update_check: true,
    owns_terminal: true,
};
