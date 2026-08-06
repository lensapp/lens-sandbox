use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use clap::FromArgMatches;

use lns_policy::host_access_decisions::HostAccessDecisionStore;

use crate::command::{CommandSpec, RunCtx, RunFuture, subcommand};

#[derive(clap::Args)]
pub struct HostAccessArgs {
    #[command(subcommand)]
    pub command: HostAccessCommand,
}

#[derive(clap::Subcommand)]
pub enum HostAccessCommand {
    #[command(about = "List the host capabilities this machine can resolve.")]
    List,
    #[command(about = "Grant a host capability to this directory, so runs arm it without asking.")]
    Grant(HostAccessIdArgs),
    #[command(about = "Withdraw a host capability from this directory.")]
    Revoke(HostAccessIdArgs),
}

#[derive(clap::Args)]
pub struct HostAccessIdArgs {
    #[arg(help = "The host-access id, e.g. `git-signing`.")]
    pub id: String,
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(subcommand::<HostAccessArgs>("host-access").about(
        "List, grant, and revoke the host capabilities a sandbox in this directory may use.",
    ))
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "host-access",
    augment,
    run: run_command,
    announces_update_check: true,
    owns_terminal: crate::command::never_owns_terminal,
};

pub fn run_command<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = HostAccessArgs::from_arg_matches(matches)?;
        let cwd = ctx.cwd()?;
        let path = crate::run::summary::policy_path(None, &cwd);
        let verdicts = lns_policy::host_access_decisions::JsonFileHostAccessDecisionStore::new(
            lns_policy::host_access_decisions::default_host_access_decisions_path(),
        );
        let mut out = ctx.out;
        run(&args.command, &path, &verdicts, &mut out)
    })
}

pub fn run(
    cmd: &HostAccessCommand,
    policy_path: &Path,
    verdicts: &dyn HostAccessDecisionStore,
    writer: &mut impl Write,
) -> Result<i32> {
    match cmd {
        HostAccessCommand::List => list(policy_path, writer),
        HostAccessCommand::Grant(args) => grant(&args.id, policy_path, verdicts, writer),
        HostAccessCommand::Revoke(args) => revoke(&args.id, policy_path, writer),
    }
}

fn load(policy_path: &Path) -> Result<lns_policy::Policy> {
    lns_policy::Policy::load_or_default(policy_path)
        .with_context(|| format!("reading {}", policy_path.display()))
}

fn save(policy: &lns_policy::Policy, policy_path: &Path) -> Result<()> {
    policy
        .save_atomic(policy_path)
        .with_context(|| format!("writing {}", policy_path.display()))
}

fn list(policy_path: &Path, writer: &mut impl Write) -> Result<i32> {
    let granted = load(policy_path)?.host_access;
    for entry in lns_policy::host_access::bundled_host_access() {
        let state = if granted.contains(&entry.id) {
            "granted"
        } else {
            "available"
        };
        writeln!(writer, "{}\t{}\t{state}", entry.id, entry.name)?;
    }
    Ok(0)
}

fn known(id: &str) -> Result<()> {
    if lns_policy::host_access::find(id).is_none() {
        anyhow::bail!(
            "unknown host access {id:?}; run `lns host-access list` to see what this machine can resolve"
        );
    }
    Ok(())
}

fn grant(
    id: &str,
    policy_path: &Path,
    verdicts: &dyn HostAccessDecisionStore,
    writer: &mut impl Write,
) -> Result<i32> {
    known(id)?;
    let mut policy = load(policy_path)?;
    let already = policy.host_access.iter().any(|held| held == id);
    policy.grant_host_access(id.to_string());
    // A grant also clears a standing decline, otherwise the explicit yes would lose to the remembered no.
    let cleared = clear_decline(id, verdicts)?;
    save(&policy, policy_path)?;
    let note = if cleared {
        " (the standing decline on this machine is cleared)"
    } else {
        ""
    };
    if already {
        writeln!(
            writer,
            "{id} is already granted in {}{note}",
            policy_path.display()
        )?;
    } else {
        writeln!(writer, "Granted {id} in {}{note}", policy_path.display())?;
    }
    Ok(0)
}

fn revoke(id: &str, policy_path: &Path, writer: &mut impl Write) -> Result<i32> {
    let mut policy = load(policy_path)?;
    if !policy.revoke_host_access(id) {
        writeln!(writer, "{id} is not granted in {}", policy_path.display())?;
        return Ok(1);
    }
    save(&policy, policy_path)?;
    writeln!(writer, "Revoked {id} in {}", policy_path.display())?;
    Ok(0)
}

/// An explicit grant also clears a standing decline; otherwise the remembered no would beat the yes the operator just typed.
fn clear_decline(id: &str, verdicts: &dyn HostAccessDecisionStore) -> Result<bool> {
    let mut state = verdicts.load().context("reading host-access decisions")?;
    if state.remove(id).is_none() {
        return Ok(false);
    }
    verdicts
        .save(&state)
        .context("writing host-access decisions")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::RunCtx;

    /// The wiring under test is that `run_command` resolves the policy beside the given cwd; the verb behaviour itself is pinned by the behaviours feature.
    #[tokio::test]
    #[serial_test::serial(env)]
    async fn run_command_grants_against_the_policy_beside_the_working_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let decisions = dir.path().join("decisions.json");
        std::fs::write(&decisions, r#"{"git-signing":"declined"}"#).unwrap();
        let _guard = crate::test_env::EnvScope::set("LNS_HOST_ACCESS_DECISIONS_PATH", &decisions);
        let matches = crate::command::build_cli()
            .try_get_matches_from(["lns", "host-access", "grant", "git-signing"])
            .unwrap();
        let (_, sub) = matches.subcommand().unwrap();
        let mut input: &[u8] = b"";
        let mut out: Vec<u8> = Vec::new();
        let ctx = RunCtx {
            debug: false,
            cwd: Some(dir.path().to_path_buf()),
            input: &mut input,
            out: &mut out,
        };
        assert_eq!(run_command(sub, ctx).await.unwrap(), 0);
        let policy =
            lns_policy::Policy::load_or_default(&dir.path().join("lns-policy.yaml")).unwrap();
        assert_eq!(policy.host_access, ["git-signing"]);
    }
}
