mod cases;
mod config;
mod diff;
mod fixtures;
mod host;
mod lns;
mod result;
mod run;
mod sample;
mod service;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};
use config::{Config, parse_env_pair};
use fixtures::{Fixtures, Sizes, refuse_unsuitable_bind};
use result::RunResult;
use run::{BackendOverrides, RunPlan};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "parity",
    about = "Put the same guest network cases to two lens-sandbox network backends and diff the results"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Run(RunArgs),
    Diff(DiffArgs),
    Fixtures(FixtureArgs),
}

#[derive(Args)]
struct RunArgs {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    backend: String,
    #[arg(long)]
    lns: Option<PathBuf>,
    #[arg(long = "lns-service")]
    lns_service: Option<PathBuf>,
    #[arg(long = "env", value_name = "KEY=VALUE")]
    env: Vec<String>,
    #[arg(long)]
    bind: Option<Ipv4Addr>,
    #[arg(long)]
    base_port: Option<u16>,
    #[arg(long)]
    guest_subnet: Option<String>,
    #[arg(long = "case", value_name = "NAME")]
    cases: Vec<String>,
    #[arg(long)]
    work_dir: Option<PathBuf>,
    #[arg(long, short)]
    out: PathBuf,
    #[arg(long)]
    allow_any_host: bool,
}

#[derive(Args)]
struct DiffArgs {
    left: PathBuf,
    right: PathBuf,
}

#[derive(Args)]
struct FixtureArgs {
    #[arg(long)]
    bind: Ipv4Addr,
    #[arg(long, default_value_t = config::DEFAULT_BASE_PORT)]
    base_port: u16,
    #[arg(long, default_value = "fixtures.json")]
    report: PathBuf,
    #[arg(long)]
    seconds: Option<u64>,
}

fn main() -> Result<()> {
    let code = dispatch(Cli::parse())?;
    std::process::exit(code);
}

fn dispatch(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Run(args) => run_backend(args),
        Command::Diff(args) => diff_results(args),
        Command::Fixtures(args) => serve_fixtures(args),
    }
}

fn run_backend(args: RunArgs) -> Result<i32> {
    if !host::is_macos() && !args.allow_any_host {
        bail!(
            "this harness compares the macOS guest network backends; pass --allow-any-host to run it anyway"
        );
    }

    let config = match &args.config {
        Some(path) => Some(Config::load(path)?),
        None => None,
    };
    let mut env = BTreeMap::new();
    for pair in &args.env {
        let (key, value) = parse_env_pair(pair)?;
        env.insert(key, value);
    }
    let backend = run::merge_backend(
        config.as_ref(),
        &args.backend,
        BackendOverrides {
            lns: args.lns,
            lns_service: args.lns_service,
            env,
        },
    )?;

    let bind = match args.bind.or_else(|| {
        config
            .as_ref()
            .and_then(|config| config.bind.clone())
            .and_then(|text| text.parse().ok())
    }) {
        Some(address) => address,
        None => bail!("--bind takes the host's LAN IPv4 address the guest reaches it on"),
    };

    let plan = RunPlan {
        backend,
        images: config
            .as_ref()
            .map(|c| c.images.clone())
            .unwrap_or_default(),
        bind,
        base_port: args
            .base_port
            .or_else(|| config.as_ref().map(Config::base_port))
            .unwrap_or(config::DEFAULT_BASE_PORT),
        guest_subnet: args
            .guest_subnet
            .or_else(|| config.as_ref().map(Config::guest_subnet))
            .unwrap_or_else(|| config::DEFAULT_GUEST_SUBNET.to_string()),
        selected: run::select_cases(&args.cases)?,
        work_dir: args.work_dir.unwrap_or_else(default_work_dir),
        out: args.out.clone(),
    };

    let result = run::execute(plan)?;
    if run::all_skipped(&result.cases) {
        eprintln!("[warn]  every case skipped; this run pins nothing");
    }
    println!("{}", run::summarise(&result, &args.out));
    Ok(run::exit_code(&result))
}

fn diff_results(args: DiffArgs) -> Result<i32> {
    let left = RunResult::read(&args.left)?;
    let right = RunResult::read(&args.right)?;
    let report = diff::diff(&left, &right);
    print!("{}", diff::render(&report, &left, &right));
    Ok(i32::from(!report.agrees()))
}

fn serve_fixtures(args: FixtureArgs) -> Result<i32> {
    refuse_unsuitable_bind(args.bind)?;
    let fixtures = Fixtures::start(args.bind, args.base_port, Sizes::default())?;
    let report = fixtures.report();
    for (role, port) in &report.ports {
        println!("{role}\t{}:{port}", report.bind);
    }
    println!(
        "writing {} every second; stop with ctrl-c",
        args.report.display()
    );

    let deadline = args
        .seconds
        .map(|seconds| std::time::Instant::now() + Duration::from_secs(seconds));
    loop {
        std::thread::sleep(Duration::from_secs(1));
        fixtures.write_report(&args.report)?;
        if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            return Ok(0);
        }
    }
}

fn default_work_dir() -> PathBuf {
    std::env::temp_dir().join(format!("parity-{}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_grammar_is_the_one_the_readme_documents() {
        Cli::command().debug_assert();
    }

    #[test]
    fn a_run_names_its_backend_its_binaries_and_where_the_result_lands() {
        let cli = Cli::parse_from([
            "parity",
            "run",
            "--backend",
            "netstack",
            "--lns",
            "bin/lns",
            "--lns-service",
            "bin/lns-service",
            "--env",
            "LNS_NETDEV=netstack",
            "--bind",
            "192.168.1.50",
            "--out",
            "netstack.json",
        ]);
        let Command::Run(args) = cli.command else {
            panic!("expected a run");
        };

        assert_eq!(args.backend, "netstack");
        assert_eq!(args.lns, Some(PathBuf::from("bin/lns")));
        assert_eq!(args.env, vec!["LNS_NETDEV=netstack"]);
        assert_eq!(args.bind, Some(Ipv4Addr::new(192, 168, 1, 50)));
        assert_eq!(args.out, PathBuf::from("netstack.json"));
        assert!(!args.allow_any_host);
    }

    #[test]
    fn a_run_on_a_host_that_cannot_boot_a_guest_says_so_rather_than_failing_every_case() {
        if host::is_macos() {
            return;
        }
        let args = RunArgs {
            config: None,
            backend: "netstack".into(),
            lns: Some(PathBuf::from("bin/lns")),
            lns_service: Some(PathBuf::from("bin/lns-service")),
            env: vec![],
            bind: Some(Ipv4Addr::new(192, 168, 1, 50)),
            base_port: None,
            guest_subnet: None,
            cases: vec![],
            work_dir: None,
            out: PathBuf::from("netstack.json"),
            allow_any_host: false,
        };
        let err = run_backend(args).unwrap_err().to_string();

        assert!(err.contains("--allow-any-host"), "{err}");
    }

    #[test]
    fn a_run_without_a_lan_address_is_refused_before_anything_starts() {
        let args = RunArgs {
            config: None,
            backend: "netstack".into(),
            lns: Some(PathBuf::from("bin/lns")),
            lns_service: Some(PathBuf::from("bin/lns-service")),
            env: vec![],
            bind: None,
            base_port: None,
            guest_subnet: None,
            cases: vec![],
            work_dir: None,
            out: PathBuf::from("netstack.json"),
            allow_any_host: true,
        };
        let err = run_backend(args).unwrap_err().to_string();

        assert!(err.contains("--bind"), "{err}");
    }

    #[test]
    fn an_environment_flag_that_is_not_a_pair_is_refused() {
        let args = RunArgs {
            config: None,
            backend: "netstack".into(),
            lns: Some(PathBuf::from("bin/lns")),
            lns_service: Some(PathBuf::from("bin/lns-service")),
            env: vec!["LNS_NETDEV".into()],
            bind: Some(Ipv4Addr::new(192, 168, 1, 50)),
            base_port: None,
            guest_subnet: None,
            cases: vec![],
            work_dir: None,
            out: PathBuf::from("netstack.json"),
            allow_any_host: true,
        };
        let err = run_backend(args).unwrap_err().to_string();

        assert!(err.contains("KEY=VALUE"), "{err}");
    }

    #[test]
    fn the_diff_exits_non_zero_when_the_two_runs_disagree() {
        let dir = tempfile::tempdir().unwrap();
        let left_path = dir.path().join("a.json");
        let right_path = dir.path().join("b.json");
        let mut left = sample_result("netstack", result::Status::Pass);
        left.write(&left_path).unwrap();
        left.backend.name = "vmnet".into();
        left.cases[0].status = result::Status::Fail;
        left.write(&right_path).unwrap();

        assert_eq!(
            diff_results(DiffArgs {
                left: left_path.clone(),
                right: right_path,
            })
            .unwrap(),
            1
        );
        assert_eq!(
            diff_results(DiffArgs {
                left: left_path.clone(),
                right: left_path,
            })
            .unwrap(),
            0
        );
    }

    #[test]
    fn the_fixtures_refuse_a_bind_address_no_guest_can_reach() {
        let err = serve_fixtures(FixtureArgs {
            bind: Ipv4Addr::LOCALHOST,
            base_port: 0,
            report: PathBuf::from("fixtures.json"),
            seconds: Some(1),
        })
        .unwrap_err()
        .to_string();

        assert!(err.contains("loopback"), "{err}");
    }

    #[test]
    fn the_work_directory_is_this_processs_own() {
        let dir = default_work_dir();
        assert!(
            dir.ends_with(format!("parity-{}", std::process::id())),
            "{}",
            dir.display()
        );
    }

    fn sample_result(backend: &str, status: result::Status) -> RunResult {
        let mut case = result::CaseResult::new("upload-100m");
        case.status = status;
        RunResult {
            schema_version: result::SCHEMA_VERSION,
            harness_revision: "abc".into(),
            backend: result::BackendRecord {
                name: backend.into(),
                env: BTreeMap::new(),
                expectations: BTreeMap::new(),
                expected_differences: vec![],
            },
            binaries: vec![],
            lns_version: "lns 0.25.0".into(),
            service_pid: None,
            images: vec![],
            host: result::HostFacts::default(),
            started_unix_ms: 0,
            finished_unix_ms: 0,
            cases: vec![case],
            samples: vec![],
        }
    }
}
