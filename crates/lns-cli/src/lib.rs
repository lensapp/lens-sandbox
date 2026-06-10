pub mod audit;
pub mod chord;
pub mod cli;
pub mod integration;
pub mod log;
pub mod policy;
pub mod raw_mode;
pub mod run;
pub mod sandbox;
pub mod service;
#[cfg(test)]
mod test_env;
pub mod update;
pub mod update_check;

use anyhow::Result;
use cli::{Cli, Command, LogLevel};

pub async fn run(cli: Cli) -> Result<i32> {
    let debug = cli.log_level == LogLevel::Debug;
    if update_check::should_announce(&cli.command)
        && update_check::announce_enabled(|k| std::env::var(k).ok())
    {
        let _ = update_check::real::run_announce();
    }
    let code = match cli.command {
        Command::Run(args) => {
            service::require_running().await;
            service::run_image(args, debug).await?
        }
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
    };
    Ok(code)
}
