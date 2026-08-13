use std::path::PathBuf;

use cucumber::{given, then, when};
use lns_cli::command::parse_args;
use lns_cli::run::summary::print_run_summary;
use lns_cli::sandbox::{SandboxCommand, TermInfo, run_with_writers};
use lns_ipc::{ArtifactInspection, Response, SandboxView};

use super::sandbox_cli::fake_sandbox_service;
use crate::runner::CliRun;
use crate::world::BehaviourWorld;

const TOOLS_REFERENCE: &str = "registry.example.test/team/sandbox:1";
const PINNED_TOOLS: [&str; 2] = ["node@22.11.0", "python@3.12.6"];

#[given(regex = r#"^a lns\.yaml declaring tools \[(.*)\]$"#)]
fn lns_yaml_with_tools(w: &mut BehaviourWorld, list: String) {
    let entries: String = list
        .split(',')
        .map(|entry| format!("    - {}\n", entry.trim().trim_matches('"')))
        .collect();
    w.author_files.insert(
        PathBuf::from("/work/lns.yaml"),
        format!(
            "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: ghcr.io/team/base:1\n  tools:\n{entries}"
        ),
    );
}

#[then("validation succeeds without touching the network or the service")]
fn validation_succeeds_offline(w: &mut BehaviourWorld) -> Result<(), String> {
    let res = w.result.as_ref().ok_or("no CLI run captured")?;
    if res.exit_code != 0 {
        return Err(format!(
            "expected exit code 0, got {} (output: {:?})",
            res.exit_code, res.output
        ));
    }
    if !res.output.contains("is valid") {
        return Err(format!("expected a validity verdict, got {:?}", res.output));
    }
    let requests = w.sandbox.requests.lock().unwrap();
    if requests.is_empty() {
        Ok(())
    } else {
        Err(format!("expected no service request, saw {requests:?}"))
    }
}

#[then(regex = r#"^validation fails naming the entry and the expected "name@version" shape$"#)]
fn validation_fails_naming_shape(w: &mut BehaviourWorld) -> Result<(), String> {
    let res = w.result.as_ref().ok_or("no CLI run captured")?;
    if res.exit_code == 0 {
        return Err(format!("validation passed: {:?}", res.output));
    }
    if res.output.contains(r#""node@""#) && res.output.contains(r#""name@version""#) {
        Ok(())
    } else {
        Err(format!(
            "expected the entry and the \"name@version\" shape in {:?}",
            res.output
        ))
    }
}

#[given("a published sandbox declaring tools")]
fn published_sandbox_declaring_tools(w: &mut BehaviourWorld) {
    let view = SandboxView {
        mixins: Vec::new(),
        pinned_mixins: Vec::new(),
        contributions: Vec::new(),
        reference: TOOLS_REFERENCE.into(),
        digest: format!("sha256:{}", "a".repeat(64)),
        image: "registry.example.test/runtime:1".into(),
        workdir: None,
        user: None,
        mounts: Vec::new(),
        ports: Vec::new(),
        filesets: Vec::new(),
        connectors: Vec::new(),
        env: Vec::new(),
        credentials: Vec::new(),
        tools: PINNED_TOOLS.iter().map(ToString::to_string).collect(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
    };
    w.sandbox.response = Some(Response::Error {
        message: format!("no active run with id {TOOLS_REFERENCE}"),
    });
    w.sandbox.inspect_image_response = Some(Response::ImageInspected {
        inspection: ArtifactInspection::Sandbox(Box::new(view.clone())),
    });
    w.pulled_view = Some(view);
}

#[when(regex = r#"^I run "lns inspect" on its reference$"#)]
async fn run_inspect_on_its_reference(w: &mut BehaviourWorld) {
    let svc = fake_sandbox_service(w);
    let mut out: Vec<u8> = Vec::new();
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let result = run_with_writers(
        &SandboxCommand::Inspect(lns_cli::sandbox::SandboxInspectArgs {
            run: Some(TOOLS_REFERENCE.into()),
            mixins: Vec::new(),
            file: None,
        }),
        &svc,
        TermInfo::default(),
        &mut std::io::Cursor::new(""),
        &mut out,
        &mut stdout,
        &mut stderr,
    )
    .await;
    w.result = Some(match result {
        Ok(exit_code) => CliRun {
            exit_code,
            output: String::from_utf8_lossy(&out).into_owned(),
        },
        Err(e) => CliRun {
            exit_code: 1,
            output: format!("{e:#}"),
        },
    });
}

#[then("each declared tool and its pinned version is listed")]
fn each_tool_listed(w: &mut BehaviourWorld) -> Result<(), String> {
    let res = w.result.as_ref().ok_or("no CLI run captured")?;
    for tool in PINNED_TOOLS {
        if !res.output.contains(&format!("tool: {tool}")) {
            return Err(format!("expected \"tool: {tool}\" in {:?}", res.output));
        }
    }
    Ok(())
}

#[then("the run summary discloses them at launch")]
fn run_summary_discloses_tools(w: &mut BehaviourWorld) -> Result<(), String> {
    let view = w.pulled_view.take().ok_or("a pulled view is staged")?;
    let mut args: lns_cli::cli::RunArgs =
        parse_args(["lns", "run", TOOLS_REFERENCE]).map_err(|e| format!("{e:#}"))?;
    args.tools = lns_cli::run::summary::tools_from_view(&view);
    if w.cwd.is_none() {
        w.cwd = Some(tempfile::TempDir::new().map_err(|e| e.to_string())?);
    }
    let cwd = w.cwd.as_ref().ok_or("cwd")?.path().to_path_buf();
    let mut buf = Vec::<u8>::new();
    print_run_summary(
        &args,
        lns_cli::run::summary::resolved_size(Default::default(), &args),
        &cwd,
        &mut buf,
    )
    .map_err(|e| format!("{e:#}"))?;
    let summary = String::from_utf8_lossy(&buf).into_owned();
    let expected = format!("Tools:     {}", PINNED_TOOLS.join(", "));
    if summary.contains(&expected) {
        Ok(())
    } else {
        Err(format!("expected {expected:?} in:\n{summary}"))
    }
}

#[then(
    regex = r#"^validation fails asking for an explicit version such as "node@22" or "node@latest"$"#
)]
fn validation_fails_asking_version(w: &mut BehaviourWorld) -> Result<(), String> {
    let res = w.result.as_ref().ok_or("no CLI run captured")?;
    if res.exit_code == 0 {
        return Err(format!("validation passed: {:?}", res.output));
    }
    if res
        .output
        .contains(r#"explicit version such as "node@22" or "node@latest""#)
    {
        Ok(())
    } else {
        Err(format!(
            "expected the explicit-version ask in {:?}",
            res.output
        ))
    }
}
