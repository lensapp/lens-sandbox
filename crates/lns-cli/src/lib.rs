pub mod audit;
pub mod auth;
pub mod chord;
pub mod cli;
pub mod config;
pub mod image;
pub mod integration;
pub mod log;
pub mod policy;
pub mod raw_mode;
pub mod registry;
pub mod run;
pub mod sandbox;
pub mod service;
#[cfg(test)]
mod test_env;
pub mod update;
pub mod update_check;
pub mod volume;

use anyhow::Result;
use cli::{Cli, Command, LogLevel};

async fn dispatch_run(mut args: cli::RunArgs, debug: bool) -> Result<i32> {
    args.env = run::env_file::merged_run_env(&args.env_file, &args.env)?;
    let config_path = config::default_config_path()?;
    let defaults = config::load_run_defaults(&config_path)?;
    let args = config::apply_run_defaults(args, defaults);
    service::require_running().await;
    let client = registry::RealRegistryClient::new(service::socket_path()?);
    let store = lns_policy::credentials::JsonFileCredentialStore::new(
        lns_policy::credentials::default_credentials_path(),
    );
    let available = run::resolve::available_credentials(
        &lns_policy::credentials::CredentialStore::load(&store)?,
    );
    let args = run::resolve::resolve_explicit_mounts(args, &client, &mut std::io::stderr()).await?;
    let (args, _policy_guard) =
        run::resolve::resolve_into_run_args(args, &client, &available, &mut std::io::stderr())
            .await?;
    service::run_image(args, debug).await
}

pub async fn run(cli: Cli) -> Result<i32> {
    let debug = cli.log_level == LogLevel::Debug;
    if update_check::should_announce(&cli.command)
        && update_check::announce_enabled(|k| std::env::var(k).ok())
    {
        let _ = update_check::real::run_announce();
    }
    let code = match cli.command {
        Command::Run(args) => dispatch_run(args, debug).await?,
        Command::Exec(args) => {
            service::require_running().await;
            service::exec_image(args).await?
        }
        Command::Kill(args) => {
            service::require_running().await;
            service::kill(args).await?;
            0
        }
        Command::Ls => {
            service::require_running().await;
            service::ls().await?;
            0
        }
        Command::Volume(args) => {
            service::require_running().await;
            let svc = volume::RealVolumeService::new(service::socket_path()?);
            let stdin = std::io::stdin();
            let mut input = stdin.lock();
            volume::run(&args.command, &svc, &mut input, &mut std::io::stdout()).await?
        }
        Command::Image(args) => {
            service::require_running().await;
            let svc = image::RealImageService::new(service::socket_path()?);
            let stdin = std::io::stdin();
            let mut input = stdin.lock();
            image::run(&args.command, &svc, &mut input, &mut std::io::stdout()).await?
        }
        Command::Sandbox(args) => {
            service::require_running().await;
            sandbox::real::dispatch(args).await?
        }
        Command::Audit(args) => audit::run_verify(args)?,
        Command::Service(args) => {
            service::dispatch(&args.command).await?;
            0
        }
        Command::Update(args) => update::run(args).await?,
        Command::Policy(args) => {
            let cwd = std::env::current_dir()?;
            policy::run(&args.command, &cwd, &mut std::io::stdout())?
        }
        Command::Push(args) => {
            let cwd = std::env::current_dir()?;
            service::require_running().await;
            let client = registry::RealRegistryClient::new(service::socket_path()?);
            registry::push(&args, &cwd, &client, &mut std::io::stdout()).await?
        }
        Command::Pull(args) => {
            service::require_running().await;
            let client = registry::RealRegistryClient::new(service::socket_path()?);
            registry::pull(&args, &client, &mut std::io::stdout()).await?
        }
        Command::Login(args) => {
            let store = lns_policy::registry_auth::JsonFileRegistryCredentialStore::new(
                lns_policy::registry_auth::default_registry_auth_path(),
            );
            let stdin = std::io::stdin();
            let mut input = stdin.lock();
            auth::login(&args, &store, &mut input, &mut std::io::stdout())?
        }
        Command::Logout(args) => {
            let store = lns_policy::registry_auth::JsonFileRegistryCredentialStore::new(
                lns_policy::registry_auth::default_registry_auth_path(),
            );
            auth::logout(&args, &store, &mut std::io::stdout())?
        }
        Command::Auth(args) => {
            let store = lns_policy::registry_auth::JsonFileRegistryCredentialStore::new(
                lns_policy::registry_auth::default_registry_auth_path(),
            );
            auth::run(&args.command, &store, &mut std::io::stdout())?
        }
        Command::Integration(args) => {
            let cwd = std::env::current_dir()?;
            let catalog_path = lns_policy::integrations::default_integrations_path();
            let signin = integration::RealIntegrationSignIn::new(service::socket_path()?);
            integration::run(
                &args.command,
                &cwd,
                &catalog_path,
                &signin,
                &mut std::io::stdout(),
            )
            .await?
        }
        Command::Config(args) => {
            let path = config::default_config_path()?;
            config::run(&args.command, &path, &mut std::io::stdout())?
        }
    };
    Ok(code)
}
