pub mod audit;
pub mod build;
pub mod chord;
pub mod cli;
pub mod command;
pub mod config;
pub mod integration;
pub mod log;
pub mod login;
pub mod policy;
pub mod raw_mode;
pub mod run;
pub mod sandbox;
pub mod service;
#[cfg(test)]
mod test_env;
pub mod update;
pub mod update_check;
pub mod volume;

use anyhow::Result;

pub use command::build_cli;

pub async fn run_from_matches(matches: clap::ArgMatches, debug: bool) -> Result<i32> {
    let (name, sub) = matches
        .subcommand()
        .ok_or_else(|| anyhow::anyhow!("no subcommand given"))?;
    let spec =
        command::spec_for(name).ok_or_else(|| anyhow::anyhow!("unknown command `{name}`"))?;
    if spec.announces_update_check && update_check::announce_enabled(|k| std::env::var(k).ok()) {
        let _ = update_check::real::run_announce();
    }
    if spec.owns_terminal {
        // run/exec drive the tty over tokio stdin/stdout, whose blocking threads take the std stdin/stdout locks; holding those locks here would deadlock the interactive session.
        let mut input = std::io::empty();
        let mut out = std::io::sink();
        let ctx = command::RunCtx {
            debug,
            cwd: None,
            input: &mut input,
            out: &mut out,
        };
        return (spec.run)(sub, ctx).await;
    }
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let ctx = command::RunCtx {
        debug,
        cwd: None,
        input: &mut input,
        out: &mut out,
    };
    (spec.run)(sub, ctx).await
}
