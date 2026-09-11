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
    app.subcommand(
        subcommand::<crate::cli::ExecArgs>("exec").about(
            "Run another command inside a running sandbox (shortcut for `lns sandbox exec`).",
        ),
    )
}

pub const EXEC_SPEC: CommandSpec = CommandSpec {
    name: "exec",
    augment: augment_exec,
    run: crate::service::exec_command,
    announces_update_check: true,
    owns_terminal: crate::command::always_owns_terminal,
};

/// What lns-service reads from its own environment. Neither travels over the IPC a run is asked for on.
const SERVICE_VARIABLES: [&str; 2] = ["LNS_NETDEV", "LNS_GUEST_SUBNET"];

pub fn service_variable_warning(
    env_get: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Option<String> {
    let set: Vec<&str> = SERVICE_VARIABLES
        .into_iter()
        .filter(|key| env_get(key).is_some())
        .collect();
    if set.is_empty() {
        return None;
    }
    Some(format!(
        "{} takes effect on lns-service, not on this run. Set it where the service starts: \
         `lns service stop`, then start the service again with the variable in its environment.",
        set.join(" and ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn warning_when(set: &[&str]) -> Option<String> {
        let set: Vec<String> = set.iter().map(ToString::to_string).collect();
        service_variable_warning(|key| set.iter().any(|k| k == key).then(|| OsString::from("x")))
    }

    #[test]
    fn a_run_that_sets_nothing_of_the_services_is_not_warned_about() {
        assert_eq!(warning_when(&[]), None);
        assert_eq!(warning_when(&["LNS_DEBUG"]), None);
    }

    #[test]
    fn a_service_variable_set_for_the_run_says_where_it_belongs() {
        let warning = warning_when(&["LNS_NETDEV"]).expect("a variable that decides nothing here");
        assert!(warning.contains("LNS_NETDEV"), "{warning}");
        assert!(!warning.contains("LNS_GUEST_SUBNET"), "{warning}");
        assert!(warning.contains("lns service stop"), "names the remedy");
    }

    #[test]
    fn both_service_variables_are_named_in_one_line() {
        let warning = warning_when(&["LNS_GUEST_SUBNET", "LNS_NETDEV"]).expect("both are set");
        assert_eq!(
            warning.lines().count(),
            1,
            "one warning, however many are set"
        );
        assert!(
            warning.contains("LNS_NETDEV and LNS_GUEST_SUBNET"),
            "{warning}"
        );
    }
}
