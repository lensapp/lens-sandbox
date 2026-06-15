use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use anyhow::Result;
use clap::{ArgMatches, CommandFactory};

use crate::cli::Cli;

pub type RunFuture<'a> = Pin<Box<dyn Future<Output = Result<i32>> + 'a>>;

/// Real-I/O primitives a command's wiring draws from; faked in unit tests of the measured commands.
pub struct RunCtx<'a> {
    pub debug: bool,
    pub cwd: PathBuf,
    pub input: &'a mut dyn std::io::BufRead,
    pub out: &'a mut dyn std::io::Write,
}

/// One `lns <name>` subcommand, declared entirely in its own module and registered in `registry()`.
pub struct CommandSpec {
    pub name: &'static str,
    pub augment: fn(clap::Command) -> clap::Command,
    pub run: for<'a> fn(&'a ArgMatches, RunCtx<'a>) -> RunFuture<'a>,
    pub announces_update_check: bool,
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
}
