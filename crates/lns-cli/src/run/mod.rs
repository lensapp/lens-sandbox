pub mod declarative;
pub mod env_file;
pub mod host_bind;
pub mod host_path_consent;
pub mod progress;
pub mod pull_confirm;
pub mod summary;
pub mod target;

use crate::command::{CommandSpec, subcommand};

// `-h` is the hostname flag, so run offers help through `--help` alone.
pub fn long_help_only(cmd: clap::Command) -> clap::Command {
    cmd.disable_help_flag(true).arg(
        clap::Arg::new("help")
            .long("help")
            .action(clap::ArgAction::Help)
            .help("Print help"),
    )
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(long_help_only(
        subcommand::<crate::cli::RunArgs>("run").about("Run a sandbox in a microVM."),
    ))
}

pub const RUN_SPEC: CommandSpec = CommandSpec {
    name: "run",
    augment,
    run: crate::service::run_command,
    announces_update_check: true,
    owns_terminal: crate::command::always_owns_terminal,
};

pub fn augment_exec(app: clap::Command) -> clap::Command {
    app.subcommand(subcommand::<crate::cli::ExecArgs>("exec").hide(true))
}

pub const EXEC_SPEC: CommandSpec = CommandSpec {
    name: "exec",
    augment: augment_exec,
    run: crate::service::exec_command,
    announces_update_check: true,
    owns_terminal: crate::command::always_owns_terminal,
};
