use std::path::PathBuf;

use cucumber::{given, then, when};
use lns_cli::run::declarative::{Defaults, resolve};

use crate::world::{BehaviourWorld, TEST_HOST};

#[when(regex = r#"^the user runs "lns push(.*)"$"#)]
async fn user_runs_lns_push(w: &mut BehaviourWorld, rest: String) {
    super::sandbox_cli::drive_sandbox_command(w, &format!("push{rest}")).await;
}

#[then(regex = r#"^the pushed artifact carries the definition from "([^"]+)"$"#)]
fn pushed_artifact_carries_variant(w: &mut BehaviourWorld, _name: String) -> Result<(), String> {
    let doc = w.pushed_doc.as_ref().ok_or("no definition was pushed")?;
    let value: serde_json::Value =
        serde_json::from_slice(doc).map_err(|e| format!("pushed doc is not json: {e}"))?;
    if value["spec"]["image"] != "ghcr.io/team/dev-base:1" {
        return Err(format!(
            "expected the variant definition to publish, got image {}",
            value["spec"]["image"]
        ));
    }
    Ok(())
}

const VARIANT_YAML: &str = "apiVersion: lns.run/v1\nkind: sandbox\nname: dev-variant\nspec:\n  image: ghcr.io/team/dev-base:1\n";
const DEFAULT_YAML: &str =
    "apiVersion: lns.run/v1\nkind: sandbox\nname: sandbox\nspec:\n  image: ghcr.io/team/base:1\n";

#[given(regex = r#"^a sandbox definition file "([^"]+)" in the current directory$"#)]
fn variant_definition_in_cwd(w: &mut BehaviourWorld, name: String) {
    w.author_files
        .insert(PathBuf::from("/work/lns.yaml"), DEFAULT_YAML.to_string());
    w.author_files
        .insert(PathBuf::from("/work").join(name), VARIANT_YAML.to_string());
}

#[given(regex = r#"^a sandbox definition file "([^"]+)" declaring a relative bind and fileset$"#)]
fn variant_definition_with_relative_sources(w: &mut BehaviourWorld, path: String) {
    let path = PathBuf::from(path);
    let dir = path
        .parent()
        .expect("the fixture path names a file in a directory")
        .to_path_buf();
    let yaml = "apiVersion: lns.run/v1\nkind: sandbox\nname: dev-variant\nspec:\n  image: ghcr.io/team/dev-base:1\n  volumes:\n    - type: bind\n      source: .\n      target: /workspace\n  filesets:\n    - path: ./skills\n      guestPath: /root/.agent/skills\n";
    w.author_files.insert(path, yaml.to_string());
    w.author_files
        .insert(dir.join("skills/prompts.md"), "p".to_string());
}

fn wire_definition(w: &BehaviourWorld) -> Result<serde_json::Value, String> {
    let json = w
        .sandbox_run
        .definition
        .as_deref()
        .ok_or("the run request carried no definition")?;
    serde_json::from_str(json).map_err(|e| format!("definition was not json: {e}"))
}

#[then(regex = r#"^the service request carries the definition from "([^"]+)"$"#)]
fn request_carries_variant_definition(w: &mut BehaviourWorld, _name: String) -> Result<(), String> {
    let value = wire_definition(w)?;
    if value["spec"]["image"] != "ghcr.io/team/dev-base:1" {
        return Err(format!(
            "expected the variant definition on the wire, got image {}",
            value["spec"]["image"]
        ));
    }
    Ok(())
}

#[then(regex = r#"^the run reads its decisions from "([^"]+)"$"#)]
fn run_reads_decisions_from(w: &mut BehaviourWorld, path: String) -> Result<(), String> {
    let found = w
        .sandbox_run
        .decisions
        .as_ref()
        .ok_or("the run resolved no decisions file")?;
    if found != &PathBuf::from(&path) {
        return Err(format!(
            "one directory is one project, so a definition somewhere else is governed by the decisions beside it; expected {path}, got {}",
            found.display()
        ));
    }
    Ok(())
}

#[then(regex = r#"^the service request roots the bind and fileset at "([^"]+)"$"#)]
fn request_roots_sources_at(w: &mut BehaviourWorld, dir: String) -> Result<(), String> {
    let value = wire_definition(w)?;
    let fileset = value["spec"]["filesets"][0]["path"]
        .as_str()
        .unwrap_or_default();
    if fileset != format!("{dir}/skills") {
        return Err(format!(
            "expected the fileset rooted at {dir}, got {fileset:?}"
        ));
    }
    let json = w.sandbox_run.definition.as_deref().unwrap_or_default();
    let def = lns_artifact::sandbox::parse(json.as_bytes())
        .map_err(|e| format!("the wire definition did not parse: {e:#}"))?;
    let project = w
        .sandbox_run
        .project_dir
        .clone()
        .ok_or("the run resolved no project directory")?;
    let resolved = resolve(
        &Defaults::from_definition(&def, Some(TEST_HOST)),
        &project,
        None,
        None,
        Vec::new(),
    )
    .map_err(|e| format!("declarative settings did not resolve: {e:#}"))?;
    let (_, binds) = lns_cli::cli::split_mounts(&resolved.mounts);
    let rendered: Vec<String> = binds
        .iter()
        .map(|b| format!("{} -> {}", b.host_source, b.target))
        .collect();
    if rendered != [format!("{dir} -> /workspace")] {
        return Err(format!(
            "expected the bind rooted at {dir}, got {rendered:?}"
        ));
    }
    Ok(())
}
