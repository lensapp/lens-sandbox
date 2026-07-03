#[derive(Debug, Clone)]
pub struct CliRun {
    pub exit_code: i32,
    pub output: String,
}

pub fn run_lns(args: &[&str]) -> CliRun {
    let mut argv: Vec<&str> = vec!["lns"];
    argv.extend(args);
    match lns_cli::command::try_get_matches_from(argv).map(|_| ()) {
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
