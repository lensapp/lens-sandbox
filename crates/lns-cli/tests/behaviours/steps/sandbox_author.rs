use std::path::PathBuf;

use cucumber::{given, then};

use crate::world::BehaviourWorld;

const EXISTING_SENTINEL: &str =
    "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: kept\nspec:\n  image: kept:1\n";

fn yaml_key() -> PathBuf {
    PathBuf::from("/work/lns.yaml")
}

fn seed(w: &mut BehaviourWorld, contents: &str) {
    w.author_files.insert(yaml_key(), contents.to_string());
}

#[given("the current directory has no lns.yaml")]
fn no_lns_yaml(w: &mut BehaviourWorld) {
    w.author_files.clear();
}

#[given("the current directory already has an lns.yaml")]
fn existing_lns_yaml(w: &mut BehaviourWorld) {
    seed(w, EXISTING_SENTINEL);
}

#[given("a valid lns.yaml in the current directory")]
fn valid_lns_yaml(w: &mut BehaviourWorld) {
    seed(
        w,
        "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: ghcr.io/team/base:1\n",
    );
}

#[given("an lns.yaml whose env carries a value shaped like a real secret")]
fn secret_lns_yaml(w: &mut BehaviourWorld) {
    let yaml = format!(
        "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: ghcr.io/team/base:1\n  env:\n    GH_TOKEN: ghp_{}\n",
        "a".repeat(36)
    );
    seed(w, &yaml);
}

#[then(regex = r#"^a file "lns\.yaml" is created$"#)]
fn file_created(w: &mut BehaviourWorld) -> Result<(), String> {
    if w.author_files.contains_key(&yaml_key()) {
        Ok(())
    } else {
        Err("lns.yaml was not created".to_string())
    }
}

#[then(regex = r#"^the file "lns\.yaml" contains "([^"]+)"$"#)]
fn file_contains(w: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    let contents = w
        .author_files
        .get(&yaml_key())
        .ok_or("lns.yaml does not exist")?;
    if contents.contains(&needle) {
        Ok(())
    } else {
        Err(format!(
            "expected lns.yaml to contain {needle:?}, got:\n{contents}"
        ))
    }
}

#[then("the existing lns.yaml is left unchanged")]
fn existing_unchanged(w: &mut BehaviourWorld) -> Result<(), String> {
    match w.author_files.get(&yaml_key()) {
        Some(contents) if contents == EXISTING_SENTINEL => Ok(()),
        Some(other) => Err(format!("lns.yaml was modified: {other:?}")),
        None => Err("lns.yaml was removed".to_string()),
    }
}

#[then("the service received no request")]
fn no_service_request(w: &mut BehaviourWorld) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests.is_empty() {
        Ok(())
    } else {
        Err(format!("expected no service request, saw {requests:?}"))
    }
}
