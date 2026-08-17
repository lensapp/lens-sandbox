use cucumber::{given, then};
use lns_ipc::{Request, Response};

use crate::world::BehaviourWorld;

#[given("the registry accepts the push")]
fn registry_accepts_push(w: &mut BehaviourWorld) {
    w.push_outcome = Some(Ok(format!("sha256:{}", "a".repeat(64))));
}

#[given(regex = r#"^the version index resolves "([^"]+)" to "([^"]+)"$"#)]
fn version_index_resolves(w: &mut BehaviourWorld, spec: String, exact: String) {
    w.tool_index.insert(spec, exact);
}

#[given(regex = r#"^the version index does not list "([^"]+)"$"#)]
fn version_index_does_not_list(w: &mut BehaviourWorld, spec: String) {
    w.unlisted_pins.insert(spec);
}

#[then("the published artifact carries the exact resolved versions")]
fn published_carries_exact_versions(w: &mut BehaviourWorld) -> Result<(), String> {
    let doc = w.pushed_doc.as_ref().ok_or("no definition was pushed")?;
    let value: serde_json::Value =
        serde_json::from_slice(doc).map_err(|e| format!("invalid pushed doc: {e}"))?;
    let tools = value["spec"]["tools"]
        .as_array()
        .ok_or("the pushed config carries no spec.tools")?;
    for (spec, exact) in &w.tool_index {
        let name = spec.split('@').next().unwrap_or_default();
        let pinned = format!("{name}@{exact}");
        if !tools.iter().any(|t| t.as_str() == Some(pinned.as_str())) {
            return Err(format!("expected {pinned:?} among {tools:?}"));
        }
        if tools.iter().any(|t| t.as_str() == Some(spec.as_str())) {
            return Err(format!("the fuzzy entry {spec:?} survived in {tools:?}"));
        }
    }
    Ok(())
}

#[given("the stored credential for the registry lacks push scope")]
fn credential_lacks_push_scope(w: &mut BehaviourWorld) {
    w.push_outcome = Some(Err("credential for ghcr.io lacks push scope".to_string()));
}

#[given(
    regex = r#"^a valid lns\.yaml in the current directory declaring fileset "([^"]+)" mounted at "([^"]+)"$"#
)]
fn valid_lns_yaml_with_fileset(w: &mut BehaviourWorld, path: String, mount: String) {
    w.author_files.insert(
        std::path::PathBuf::from("/work/lns.yaml"),
        format!(
            "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n  filesets:\n    - path: {path}\n      mountPath: {mount}\n"
        ),
    );
}

#[given(
    regex = r#"^a valid lns\.yaml in the current directory declaring an inline fileset at \"([^\"]+)\"$"#
)]
fn valid_lns_yaml_with_inline_fileset(w: &mut BehaviourWorld, mount: String) {
    w.author_files.insert(
        std::path::PathBuf::from("/work/lns.yaml"),
        format!(
            "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n  filesets:\n    - inline:\n        settings.json: do-not-print\n      mountPath: {mount}\n"
        ),
    );
}

#[then("a FileSet artifact is pushed alongside the sandbox")]
fn fileset_pushed(w: &mut BehaviourWorld) -> Result<(), String> {
    if w.pushed_filesets
        .iter()
        .any(|reference| reference.contains("@sha256:"))
    {
        Ok(())
    } else {
        Err(format!(
            "expected a digest-addressed fileset push, saw {:?}",
            w.pushed_filesets
        ))
    }
}

#[then("only the sandbox artifact is pushed")]
fn only_sandbox_pushed(w: &mut BehaviourWorld) -> Result<(), String> {
    if w.pushed_filesets.is_empty() && w.pushed_doc.is_some() {
        Ok(())
    } else {
        Err(format!(
            "expected only a sandbox artifact, saw filesets {:?} and sandbox={}",
            w.pushed_filesets,
            w.pushed_doc.is_some()
        ))
    }
}

#[then("the published sandbox config carries the inline content unchanged")]
fn published_config_keeps_inline(w: &mut BehaviourWorld) -> Result<(), String> {
    let doc = w.pushed_doc.as_ref().ok_or("no definition was pushed")?;
    let value: serde_json::Value =
        serde_json::from_slice(doc).map_err(|error| format!("invalid pushed json: {error}"))?;
    let entry = &value["spec"]["filesets"][0];
    if entry["inline"]["settings.json"] == "do-not-print"
        && entry.get("path").is_none()
        && entry.get("ref").is_none()
    {
        Ok(())
    } else {
        Err(format!("expected unchanged inline entry, got {entry}"))
    }
}

#[then("the published sandbox config carries the fileset as a digest-pinned ref, not a path")]
fn published_config_pins_fileset(w: &mut BehaviourWorld) -> Result<(), String> {
    let doc = w.pushed_doc.as_ref().ok_or("no definition was pushed")?;
    let value: serde_json::Value =
        serde_json::from_slice(doc).map_err(|e| format!("pushed doc is not json: {e}"))?;
    let entry = &value["spec"]["filesets"][0];
    let reference = entry["ref"].as_str().unwrap_or_default();
    if entry.get("path").is_none() && reference.contains("@sha256:") {
        Ok(())
    } else {
        Err(format!("expected a pinned ref-only entry, got: {entry}"))
    }
}

#[then("nothing is pushed")]
fn nothing_pushed(w: &mut BehaviourWorld) -> Result<(), String> {
    if w.pushed_filesets.is_empty() && w.pushed_doc.is_none() {
        Ok(())
    } else {
        Err(format!(
            "expected no uploads, saw filesets {:?} and doc {:?}",
            w.pushed_filesets,
            w.pushed_doc.is_some()
        ))
    }
}

#[given(regex = r#"^the sandbox "([^"]+)" is cached$"#)]
fn sandbox_is_cached(w: &mut BehaviourWorld, reference: String) {
    w.sandbox.response = Some(Response::ImageTagged {
        from: reference.clone(),
        to: reference,
    });
}

#[then(regex = r#"^the service received a request to pull "([^"]+)"$"#)]
fn service_received_pull(w: &mut BehaviourWorld, reference: String) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests
        .iter()
        .any(|r| matches!(r, Request::PullImage { image, .. } if *image == reference))
    {
        Ok(())
    } else {
        Err(format!(
            "expected a PullImage request for {reference:?} among {requests:?}"
        ))
    }
}

#[then(regex = r#"^the service received a request to tag from "([^"]+)"$"#)]
fn service_received_tag_from(w: &mut BehaviourWorld, from: String) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests
        .iter()
        .any(|r| matches!(r, Request::TagImage { from: f, .. } if *f == from))
    {
        Ok(())
    } else {
        Err(format!(
            "expected a TagImage request sourced at {from:?} among {requests:?}"
        ))
    }
}

#[then(regex = r#"^the sandbox "([^"]+)" resolves to the same cached artifact$"#)]
fn sandbox_resolves_to_same_artifact(w: &mut BehaviourWorld, to: String) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests
        .iter()
        .any(|r| matches!(r, Request::TagImage { to: t, .. } if *t == to))
    {
        Ok(())
    } else {
        Err(format!(
            "expected a TagImage request tagging {to:?} among {requests:?}"
        ))
    }
}

#[given(
    regex = r#"^a valid lns\.yaml in the current directory declaring a hostPath fileset "([^"]+)" mounted at "([^"]+)"$"#
)]
fn valid_lns_yaml_with_host_path_fileset(w: &mut BehaviourWorld, source: String, mount: String) {
    w.author_files.insert(
        std::path::PathBuf::from("/work/lns.yaml"),
        format!(
            "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n  filesets:\n    - hostPath: {source}\n      mountPath: {mount}\n      optional: true\n"
        ),
    );
}

#[then("the published sandbox config carries the hostPath unchanged")]
fn published_config_keeps_host_path(w: &mut BehaviourWorld) -> Result<(), String> {
    let doc = w.pushed_doc.as_ref().ok_or("no definition was pushed")?;
    let value: serde_json::Value =
        serde_json::from_slice(doc).map_err(|error| format!("invalid pushed json: {error}"))?;
    let entry = &value["spec"]["filesets"][0];
    if entry["hostPath"] == "~/.gitconfig" && entry.get("ref").is_none() {
        Ok(())
    } else {
        Err(format!(
            "a hostPath is what makes the artifact portable — packing or rewriting it would pin the author's machine, got {entry}"
        ))
    }
}
