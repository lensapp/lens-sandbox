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

#[given("a document whose spec.image names a Containerfile")]
fn a_document_naming_a_containerfile(w: &mut BehaviourWorld) {
    w.author_files
        .insert(PathBuf::from("/work/lns.yaml"), document());
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
            } => Some((definition.clone(), definition_dir.clone(), *rebuild)),
            _ => None,
        })
        .ok_or_else(|| "no build reached the service".to_string())
}

#[then(regex = r#"^the service was asked to build "([^"]+)"$"#)]
fn the_service_was_asked_to_build(w: &mut BehaviourWorld, path: String) -> Result<(), String> {
    let (definition, dir, _) = build_request(w)?;
    let expected = PathBuf::from(&path);
    let expected_dir = expected.parent().expect("the fixture path names a file");
    if dir != expected_dir.to_string_lossy() {
        return Err(format!("the build was rooted at {dir}, not {expected_dir:?}"));
    }
    let value: serde_json::Value =
        serde_json::from_str(&definition).map_err(|e| format!("definition was not json: {e}"))?;
    if value["spec"]["image"] != "./image" {
        return Err(format!("the build carried {value:?}"));
    }
    Ok(())
}

#[then("the build request ignores the cache")]
fn the_build_ignores_the_cache(w: &mut BehaviourWorld) -> Result<(), String> {
    match build_request(w)?.2 {
        true => Ok(()),
        false => Err("the build did not ask the service to ignore the cache".to_string()),
    }
}
