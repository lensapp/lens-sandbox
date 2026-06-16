use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use anyhow::{Context, Result};
use clap::{ArgMatches, CommandFactory};

use crate::cli::Cli;

pub type RunFuture<'a> = Pin<Box<dyn Future<Output = Result<i32>> + 'a>>;

/// Real-I/O primitives a command's wiring draws from; faked in unit tests of the measured commands.
pub struct RunCtx<'a> {
    pub debug: bool,
    pub cwd: Option<PathBuf>,
    pub input: &'a mut dyn std::io::BufRead,
    pub out: &'a mut dyn std::io::Write,
}

impl RunCtx<'_> {
    /// The directory a command edits relative to: the injected one in tests, else the process cwd.
    pub fn cwd(&self) -> Result<PathBuf> {
        match &self.cwd {
            Some(cwd) => Ok(cwd.clone()),
            None => std::env::current_dir().context("reading current directory"),
        }
    }
}

/// One `lns <name>` subcommand, declared entirely in its own module and registered in `registry()`.
pub struct CommandSpec {
    pub name: &'static str,
    pub augment: fn(clap::Command) -> clap::Command,
    pub run: for<'a> fn(&'a ArgMatches, RunCtx<'a>) -> RunFuture<'a>,
    pub announces_update_check: bool,
    /// True for commands that drive the tty over async tokio stdin/stdout, so the dispatcher must not hold the blocking std stdin/stdout locks.
    pub owns_terminal: bool,
}

/// Add a derive-`Args` subcommand named `name` to `app`.
pub fn subcommand<A: clap::Args>(name: &'static str) -> clap::Command {
    A::augment_args(clap::Command::new(name))
}

pub fn registry() -> Vec<CommandSpec> {
    vec![
        crate::run::RUN_SPEC,
        crate::run::EXEC_SPEC,
        crate::service::KILL_SPEC,
        crate::service::LS_SPEC,
        crate::volume::SPEC,
        crate::image::SPEC,
        crate::sandbox::SPEC,
        crate::audit::SPEC,
        crate::service::SPEC,
        crate::update::SPEC,
        crate::policy::SPEC,
        crate::integration::SPEC,
        crate::config::SPEC,
        crate::login::LOGIN_SPEC,
        crate::login::LOGOUT_SPEC,
    ]
}

pub fn build_cli() -> clap::Command {
    let mut app = Cli::command()
        .subcommand_required(true)
        .arg_required_else_help(true);
    for spec in registry() {
        app = (spec.augment)(app);
    }
    app
}

pub fn spec_for(name: &str) -> Option<CommandSpec> {
    registry().into_iter().find(|spec| spec.name == name)
}

/// Parse `argv` (program name first) and decode the matched subcommand into its typed args.
pub fn parse_args<A, I, T>(argv: I) -> clap::error::Result<A>
where
    A: clap::FromArgMatches,
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let matches = build_cli().try_get_matches_from(argv)?;
    let (_, sub) = matches
        .subcommand()
        .expect("build_cli requires a subcommand, so a successful parse always has one");
    A::from_arg_matches(sub)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn cwd_prefers_an_injected_directory() {
        let mut input: &[u8] = b"";
        let mut out: Vec<u8> = Vec::new();
        let ctx = RunCtx {
            debug: false,
            cwd: Some(PathBuf::from("/injected")),
            input: &mut input,
            out: &mut out,
        };
        assert_eq!(ctx.cwd().unwrap(), PathBuf::from("/injected"));
    }

    #[test]
    fn cwd_falls_back_to_the_process_directory_when_unset() {
        let mut input: &[u8] = b"";
        let mut out: Vec<u8> = Vec::new();
        let ctx = RunCtx {
            debug: false,
            cwd: None,
            input: &mut input,
            out: &mut out,
        };
        assert_eq!(ctx.cwd().unwrap(), std::env::current_dir().unwrap());
    }

    #[test]
    fn every_spec_name_is_unique() {
        let mut seen = BTreeSet::new();
        for spec in registry() {
            assert!(
                seen.insert(spec.name),
                "duplicate command name {}",
                spec.name
            );
        }
    }

    #[test]
    fn build_cli_exposes_one_subcommand_per_registered_spec() {
        let app = build_cli();
        let names: BTreeSet<&str> = app.get_subcommands().map(|c| c.get_name()).collect();
        for spec in registry() {
            assert!(
                names.contains(spec.name),
                "build_cli is missing subcommand {}",
                spec.name
            );
        }
    }

    #[test]
    fn only_update_opts_out_of_the_update_check_announce() {
        for spec in registry() {
            let opts_in = spec.announces_update_check;
            if spec.name == "update" {
                assert!(
                    !opts_in,
                    "update must skip the announce so it does not nag mid-upgrade"
                );
            } else {
                assert!(opts_in, "{} should announce available updates", spec.name);
            }
        }
    }

    #[test]
    fn spec_for_finds_a_registered_command_and_rejects_an_unknown_one() {
        assert!(spec_for("volume").is_some());
        assert!(spec_for("does-not-exist").is_none());
    }

    #[test]
    fn only_run_and_exec_own_the_terminal() {
        for spec in registry() {
            if matches!(spec.name, "run" | "exec") {
                assert!(
                    spec.owns_terminal,
                    "{} drives the tty over tokio stdin/stdout; the dispatcher must not hold the std locks for it",
                    spec.name
                );
            } else {
                assert!(
                    !spec.owns_terminal,
                    "{} runs synchronously and relies on the dispatcher holding the std stdin/stdout locks",
                    spec.name
                );
            }
        }
    }
}
