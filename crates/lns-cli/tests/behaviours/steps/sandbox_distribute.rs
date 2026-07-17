use cucumber::{given, then};
use lns_ipc::{Request, Response};

use crate::world::BehaviourWorld;

#[given("the registry accepts the push")]
fn registry_accepts_push(w: &mut BehaviourWorld) {
    w.push_outcome = Some(Ok(format!("sha256:{}", "a".repeat(64))));
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
            "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: ghcr.io/team/base:1\n  filesets:\n    - path: {path}\n      mountPath: {mount}\n"
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
        .any(|r| matches!(r, Request::PullImage { image } if *image == reference))
    {
        Ok(())
    } else {
        Err(format!(
            "expected a PullImage request for {reference:?} among {requests:?}"
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
