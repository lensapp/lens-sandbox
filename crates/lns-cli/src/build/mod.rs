use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::FromArgMatches;

use crate::command::{CommandSpec, RunCtx, RunFuture, subcommand};

#[derive(clap::Args)]
pub struct BuildArgs {
    #[arg(
        value_name = "PATH",
        help = "Path to an artifact manifest (YAML or JSON)."
    )]
    pub path: std::path::PathBuf,
    #[arg(
        long,
        help = "Validate only: schema + cross-field guards + secret guard. (Assembly and push are not implemented yet.)"
    )]
    pub check: bool,
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<BuildArgs>("build").about(
            "Validate, and (soon) build and push, lens OCI artifacts from a local manifest.",
        ),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "build",
    augment,
    run: run_command,
    announces_update_check: true,
    owns_terminal: false,
};

pub fn run_command<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = BuildArgs::from_arg_matches(matches)?;
        if !args.check {
            bail!(
                "only `--check` is implemented so far; artifact assembly and push are coming next"
            );
        }
        let path = ctx.cwd()?.join(&args.path);
        let raw =
            std::fs::read(&path).with_context(|| format!("reading manifest {}", path.display()))?;
        let mut out = ctx.out;
        check_and_report(&raw, &args.path, &mut out)
    })
}

fn check_and_report(raw: &[u8], path: &Path, writer: &mut impl Write) -> Result<i32> {
    let display = path.display();
    let json = match serde_yaml::from_slice::<serde_json::Value>(raw) {
        Ok(value) => serde_json::to_vec(&value).context("re-serialising manifest")?,
        Err(e) => {
            writeln!(writer, "✖ {display}: not valid YAML or JSON: {e}")?;
            return Ok(1);
        }
    };
    match lns_artifact::validate::validate(&json) {
        Ok(()) => {
            writeln!(writer, "✔ {display}: valid")?;
            Ok(0)
        }
        Err(problems) => {
            writeln!(writer, "✖ {display}: {} problem(s)", problems.len())?;
            for problem in &problems {
                writeln!(writer, "  - {problem}")?;
            }
            Ok(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn report(raw: &[u8]) -> (i32, String) {
        let mut out: Vec<u8> = Vec::new();
        let code = check_and_report(raw, &PathBuf::from("bundle.yaml"), &mut out).unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    fn sandbox_yaml(base_image: &str) -> String {
        format!(
            "apiVersion: lens.dev/v1alpha1\nkind: Sandbox\nmetadata:\n  name: some-sandbox\nspec:\n  isolation: microvm\n  baseImage: {base_image}\n"
        )
    }

    #[test]
    fn a_valid_manifest_reports_ok_and_exits_zero() {
        let (code, out) =
            report(sandbox_yaml(&format!("reg/base@sha256:{}", "a".repeat(64))).as_bytes());
        assert_eq!(code, 0);
        assert!(out.contains("valid"), "got: {out}");
    }

    #[test]
    fn a_schema_problem_is_listed_and_exits_one() {
        let (code, out) = report(sandbox_yaml("reg/base:1").as_bytes());
        assert_eq!(code, 1);
        assert!(out.contains("problem(s)"), "got: {out}");
        assert!(out.contains("digest-pinned"), "got: {out}");
    }

    #[test]
    fn a_secret_in_the_manifest_is_reported() {
        let doc = format!(
            "apiVersion: lens.dev/v1alpha1\nkind: Agent\nmetadata:\n  name: some-agent\nspec:\n  command: agent\n  env:\n    GH_TOKEN: ghp_{}\n",
            "a".repeat(36)
        );
        let (code, out) = report(doc.as_bytes());
        assert_eq!(code, 1);
        assert!(out.contains("GitHub token"), "got: {out}");
    }

    #[test]
    fn an_unparseable_manifest_is_reported_not_panicked() {
        let (code, out) = report(b": : not : yaml :");
        assert_eq!(code, 1);
        assert!(out.contains("not valid YAML or JSON"), "got: {out}");
    }

    #[tokio::test]
    async fn run_command_reads_the_manifest_relative_to_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("bundle.yaml"),
            sandbox_yaml(&format!("reg/base@sha256:{}", "a".repeat(64))),
        )
        .unwrap();
        let app = crate::command::build_cli();
        let matches = app
            .try_get_matches_from(["lns", "build", "bundle.yaml", "--check"])
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
        let code = run_command(sub, ctx).await.unwrap();
        assert_eq!(code, 0);
        assert!(String::from_utf8(out).unwrap().contains("valid"));
    }

    #[tokio::test]
    async fn run_command_without_check_refuses_until_assembly_lands() {
        let app = crate::command::build_cli();
        let matches = app
            .try_get_matches_from(["lns", "build", "bundle.yaml"])
            .unwrap();
        let (_, sub) = matches.subcommand().unwrap();
        let mut input: &[u8] = b"";
        let mut out: Vec<u8> = Vec::new();
        let ctx = RunCtx {
            debug: false,
            cwd: None,
            input: &mut input,
            out: &mut out,
        };
        let err = run_command(sub, ctx).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("only `--check`"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn run_command_surfaces_a_missing_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let app = crate::command::build_cli();
        let matches = app
            .try_get_matches_from(["lns", "build", "absent.yaml", "--check"])
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
        let err = run_command(sub, ctx).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("reading manifest"),
            "got: {err:#}"
        );
    }
}
