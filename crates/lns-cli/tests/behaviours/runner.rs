use clap::{ColorChoice, CommandFactory};
use lns_cli::cli::Cli;

#[derive(Debug, Clone)]
pub struct CliRun {
    pub exit_code: i32,
    pub output: String,
}

pub fn run_lns(args: &[&str]) -> CliRun {
    let mut argv: Vec<&str> = vec!["lns"];
    argv.extend(args);
    let cmd = Cli::command().color(ColorChoice::Never);
    match cmd.try_get_matches_from(argv) {
        Ok(_matches) => CliRun {
            exit_code: 0,
            output: String::new(),
        },
        Err(e) => CliRun {
            exit_code: e.exit_code(),
            output: e.to_string(),
        },
    }
}
