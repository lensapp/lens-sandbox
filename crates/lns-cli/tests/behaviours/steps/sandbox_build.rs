use std::path::PathBuf;

use cucumber::{given, then};
use lns_ipc::{Request, Response};

use crate::world::BehaviourWorld;

const KEY: &str = "sha256:5f0b1a2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8";
const BUILT: &str =
    "lns-build.local/built@sha256:761743bb0a1c2d3e4f5061728394a5b6c7d8e9f0a1b2c3d4e5f60718293a4b5c";

fn document() -> String {
    "apiVersion: lns.run/v1\nkind: sandbox\nname: agent\nspec:\n  image: ./image\n".to_string()
}

fn document_with_a_mixin() -> String {
    format!("{}  mixins:\n    - ./project-egress.yaml\n", document())
}

const AUTHORED_EGRESS: &str = r#"{"http":[{"match":"registry.npmjs.org","verdict":"allow"}]}"#;

#[given("a document whose spec.image names a Containerfile")]
fn a_document_naming_a_containerfile(w: &mut BehaviourWorld) {
    w.author_files
        .insert(PathBuf::from("/work/lns.yaml"), document());
}

#[given("a document whose spec.image names a Containerfile and declares a mixin")]
fn a_document_declaring_a_mixin(w: &mut BehaviourWorld) {
    w.author_files
        .insert(PathBuf::from("/work/lns.yaml"), document_with_a_mixin());
}

#[given("the service resolves that document")]
fn the_service_resolves_that_document(w: &mut BehaviourWorld) {
    w.sandbox.resolve_response = Some(Response::DefinitionResolved {
        definition: serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "sandbox",
            "name": "agent",
            "spec": { "image": "./image" },
        })
        .to_string(),
        mixins: vec!["./project-egress.yaml".to_string()],
        pinned_mixins: Vec::new(),
        contributions: Vec::new(),
        authored_egress: AUTHORED_EGRESS.to_string(),
        packed_filesets: Vec::new(),
    });
}

#[given(regex = r#"^a second document "([^"]+)" whose spec\.image names a Containerfile$"#)]
fn a_second_document(w: &mut BehaviourWorld, name: String) {
    w.author_files
        .insert(PathBuf::from("/work").join(name), document());
}

#[given(regex = r"^the service builds it into (\d+) layers$")]
fn the_service_builds_it(w: &mut BehaviourWorld, layers: usize) {
    w.sandbox.response = Some(Response::SandboxBuilt {
        key: KEY.to_string(),
        reference: BUILT.to_string(),
        label: "./image/Containerfile".to_string(),
        layers,
        reused: false,
    });
}

#[given("the service answers that the key is already built")]
fn the_key_is_already_built(w: &mut BehaviourWorld) {
    w.sandbox.response = Some(Response::SandboxBuilt {
        key: KEY.to_string(),
        reference: BUILT.to_string(),
        label: "./image/Containerfile".to_string(),
        layers: 0,
        reused: true,
    });
}

#[given(regex = r#"^the service refuses the build with "(.+)"$"#)]
fn the_service_refuses(w: &mut BehaviourWorld, message: String) {
    w.sandbox.response = Some(Response::Error {
        message: message.replace("\\\"", "\""),
    });
}

fn build_request(w: &BehaviourWorld) -> Result<(String, String, bool), String> {
    w.sandbox
        .requests
        .lock()
        .unwrap()
        .iter()
        .find_map(|request| match request {
            Request::BuildSandbox {
                definition,
                definition_dir,
                rebuild,
                ..
            } => Some((definition.clone(), definition_dir.clone(), *rebuild)),
            _ => None,
        })
        .ok_or_else(|| "no build reached the service".to_string())
}

fn method_order(w: &BehaviourWorld) -> Vec<&'static str> {
    w.sandbox
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter_map(|request| match request {
            Request::ResolveDefinition { .. } => Some("resolve"),
            Request::BuildSandbox { .. } => Some("build"),
            _ => None,
        })
        .collect()
}

#[then("the service resolved the document before it built it")]
fn the_service_resolved_first(w: &mut BehaviourWorld) -> Result<(), String> {
    match method_order(w).as_slice() {
        ["resolve", "build"] => Ok(()),
        other => Err(format!("the service was asked for {other:?}")),
    }
}

#[then("the service was not asked to resolve the document")]
fn the_service_was_not_asked_to_resolve(w: &mut BehaviourWorld) -> Result<(), String> {
    match method_order(w).contains(&"resolve") {
        true => Err("a document declaring no mixin needs no resolution".to_string()),
        false => Ok(()),
    }
}

#[then("the build carries the resolved document and what its mixins authored")]
fn the_build_carries_what_was_resolved(w: &mut BehaviourWorld) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    let Some(Request::BuildSandbox {
        definition,
        authored_egress,
        ..
    }) = requests
        .iter()
        .find(|request| matches!(request, Request::BuildSandbox { .. }))
    else {
        return Err("no build reached the service".to_string());
    };
    let value: serde_json::Value =
        serde_json::from_str(definition).map_err(|e| format!("definition was not json: {e}"))?;
    if value["spec"].get("mixins").is_some() {
        return Err(format!("the build carried an unresolved document: {value}"));
    }
    if authored_egress.as_deref() != Some(AUTHORED_EGRESS) {
        return Err(format!(
            "the build carried {authored_egress:?} as the egress its mixins authored"
        ));
    }
    Ok(())
}

#[then(regex = r#"^the service was asked to build "([^"]+)"$"#)]
fn the_service_was_asked_to_build(w: &mut BehaviourWorld, path: String) -> Result<(), String> {
    let (definition, dir, _) = build_request(w)?;
    let expected = PathBuf::from(&path);
    let expected_dir = expected.parent().expect("the fixture path names a file");
    if dir != expected_dir.to_string_lossy() {
        return Err(format!(
            "the build was rooted at {dir}, not {expected_dir:?}"
        ));
    }
    let value: serde_json::Value =
        serde_json::from_str(&definition).map_err(|e| format!("definition was not json: {e}"))?;
    if value["spec"]["image"] != "./image" {
        return Err(format!("the build carried {value:?}"));
    }
    Ok(())
}

/// §3.1.1: the switch is a property of the machine, and only the request pins that it reached the service.
#[then("the build request names the host Docker daemon")]
fn the_build_request_names_the_daemon(w: &mut BehaviourWorld) -> Result<(), String> {
    match engine_asked_for(w)? {
        lns_ipc::BuildEngine::Docker { .. } => Ok(()),
        other => Err(format!("the build asked for {other:?}")),
    }
}

#[then("the build request names a build guest")]
fn the_build_request_names_a_guest(w: &mut BehaviourWorld) -> Result<(), String> {
    match engine_asked_for(w)? {
        lns_ipc::BuildEngine::Lns => Ok(()),
        other => Err(format!("the build asked for {other:?}")),
    }
}

fn engine_asked_for(w: &BehaviourWorld) -> Result<lns_ipc::BuildEngine, String> {
    w.sandbox
        .requests
        .lock()
        .unwrap()
        .iter()
        .find_map(|request| match request {
            Request::BuildSandbox { build_engine, .. } => Some(build_engine.clone()),
            _ => None,
        })
        .ok_or_else(|| "no build reached the service".to_string())
}

#[then("the build request ignores the cache")]
fn the_build_ignores_the_cache(w: &mut BehaviourWorld) -> Result<(), String> {
    match build_request(w)?.2 {
        true => Ok(()),
        false => Err("the build did not ask the service to ignore the cache".to_string()),
    }
}
