use std::path::PathBuf;

use cucumber::{given, then};

use crate::world::BehaviourWorld;

const EXISTING_SENTINEL: &str =
    "apiVersion: lns.run/v1\nkind: sandbox\nname: kept\nspec:\n  image: kept:1\n";

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

#[given("the current directory already has an lns.dev.yaml")]
fn existing_named_definition(w: &mut BehaviourWorld) {
    w.author_files.insert(
        PathBuf::from("/work/lns.dev.yaml"),
        EXISTING_SENTINEL.to_string(),
    );
}

#[given("a valid lns.yaml in the current directory")]
fn valid_lns_yaml(w: &mut BehaviourWorld) {
    seed(
        w,
        "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n",
    );
}

#[given(regex = r#"^an lns\.yaml declaring user "([^"]+)"$"#)]
fn lns_yaml_with_user(w: &mut BehaviourWorld, user: String) {
    seed(
        w,
        &format!(
            "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n  user: {user}\n"
        ),
    );
}

#[given("an lns.yaml holding a mixin document")]
fn lns_yaml_holding_a_mixin(w: &mut BehaviourWorld) {
    seed(
        w,
        "apiVersion: lns.run/v1\nkind: mixin\nname: postgres-tools\nspec:\n  tools:\n    - node@22\n",
    );
}

#[given("an lns.yaml holding a mixin document that declares an image")]
fn lns_yaml_holding_a_mixin_with_an_image(w: &mut BehaviourWorld) {
    seed(
        w,
        "apiVersion: lns.run/v1\nkind: mixin\nname: postgres-tools\nspec:\n  image: ghcr.io/team/base:1\n",
    );
}

#[given("an lns.yaml written against the retired lens.dev/v1alpha1 group")]
fn lns_yaml_from_the_retired_group(w: &mut BehaviourWorld) {
    seed(
        w,
        &format!(
            "apiVersion: lens.dev/v1alpha1\nkind: sandbox\nname: hermes\nspec:\n  isolation: microvm\n  baseImage: ghcr.io/team/base@sha256:{}\n",
            "a".repeat(64)
        ),
    );
}

#[given("an lns.yaml with a misspelled volume readOnly field")]
fn lns_yaml_with_unknown_nested_field(w: &mut BehaviourWorld) {
    seed(
        w,
        "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n  volumes:\n    - name: data\n      target: /data\n      readOlny: true\n",
    );
}

fn fileset_yaml(entries: &str) -> String {
    format!(
        "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n  filesets:\n{entries}"
    )
}

#[given(regex = r#"^an lns\.yaml declaring fileset "([^"]+)" mounted at "([^"]+)"$"#)]
fn lns_yaml_with_path_fileset(w: &mut BehaviourWorld, path: String, mount: String) {
    seed(
        w,
        &fileset_yaml(&format!("    - path: {path}\n      guestPath: {mount}\n")),
    );
}

#[given(
    regex = r#"^an lns\.yaml declaring a (bind|read-only|named) volume at "([^"]+)" and fileset "([^"]+)" mounted at "([^"]+)"$"#
)]
fn lns_yaml_with_volume_and_nested_fileset(
    w: &mut BehaviourWorld,
    kind: String,
    target: String,
    path: String,
    mount: String,
) {
    let volume = match kind.as_str() {
        "bind" => format!("    - type: bind\n      source: ./src\n      target: {target}\n"),
        "read-only" => format!("    - name: home\n      target: {target}\n      readOnly: true\n"),
        _ => format!("    - name: home\n      target: {target}\n"),
    };
    seed(
        w,
        &format!(
            "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n  volumes:\n{volume}  filesets:\n    - path: {path}\n      guestPath: {mount}\n"
        ),
    );
}

#[given(
    regex = r#"^an lns\.yaml declaring a hostPath fileset "([^"]+)" mounted at "([^"]+)" and optional$"#
)]
fn lns_yaml_with_host_path_fileset(w: &mut BehaviourWorld, source: String, mount: String) {
    seed(
        w,
        &fileset_yaml(&format!(
            "    - hostPath: {source}\n      guestPath: {mount}\n      optional: true\n"
        )),
    );
}

#[given("an lns.yaml declaring a fileset entry with no source")]
fn lns_yaml_with_sourceless_fileset(w: &mut BehaviourWorld) {
    seed(w, &fileset_yaml("    - guestPath: /root/.agent/skills\n"));
}

#[given("an lns.yaml declaring a fileset entry that names another artifact by ref")]
fn lns_yaml_with_a_ref_fileset(w: &mut BehaviourWorld) {
    seed(
        w,
        &fileset_yaml(
            "    - ref: registry.example.test/team/skills@sha256:abc\n      guestPath: /root/.agent/skills\n",
        ),
    );
}

fn inline_fileset_yaml(path: &str, mount: &str, owner: Option<&str>, content: &str) -> String {
    let owner = owner
        .map(|value| format!("      owner: {value}\n"))
        .unwrap_or_default();
    fileset_yaml(&format!(
        "    - inline:\n        {path}: |-\n          {content}\n      guestPath: {mount}\n{owner}"
    ))
}

#[given(
    regex = r#"^an lns\.yaml declaring an inline fileset with \"([^\"]+)\" at \"([^\"]+)\" owned by the workload$"#
)]
fn lns_yaml_with_workload_inline_fileset(w: &mut BehaviourWorld, path: String, mount: String) {
    seed(
        w,
        &inline_fileset_yaml(&path, &mount, Some("workload"), "INLINE_CONTENT"),
    );
}

#[given(regex = r#"^the inline file contains `([^`]*)`$"#)]
fn inline_file_contains(w: &mut BehaviourWorld, content: String) {
    let yaml = w
        .author_files
        .get_mut(&yaml_key())
        .expect("inline definition");
    *yaml = yaml.replace("INLINE_CONTENT", &content);
}

#[given("an lns.yaml declaring a fileset entry with inline content and path")]
fn lns_yaml_with_inline_and_path(w: &mut BehaviourWorld) {
    seed(
        w,
        &fileset_yaml(
            "    - path: ./skills\n      inline:\n        settings.json: '{}'\n      guestPath: /home/sandbox\n",
        ),
    );
}

#[given(
    regex = r#"^an lns\.yaml declaring an inline fileset with path \"([^\"]+)\" at \"([^\"]+)\"$"#
)]
fn lns_yaml_with_inline_path(w: &mut BehaviourWorld, path: String, mount: String) {
    seed(w, &inline_fileset_yaml(&path, &mount, None, "x"));
}

#[given(regex = r#"^an lns\.yaml declaring an inline fileset with \"([^\"]+)\" at \"([^\"]+)\"$"#)]
fn lns_yaml_with_inline_file(w: &mut BehaviourWorld, path: String, mount: String) {
    seed(w, &inline_fileset_yaml(&path, &mount, None, "x"));
}

#[given(regex = r#"^an lns\.yaml declaring two inline files at \"([^\"]+)\"$"#)]
fn lns_yaml_with_two_inline_files(w: &mut BehaviourWorld, mount: String) {
    seed(
        w,
        &fileset_yaml(&format!(
            "    - inline:\n        accepted.json: EXACT_CONTENT\n        oversized.json: OVERSIZED_CONTENT\n      guestPath: {mount}\n"
        )),
    );
}

#[given("one inline file is exactly 131072 bytes")]
fn inline_file_at_limit(w: &mut BehaviourWorld) {
    let yaml = w.author_files.get_mut(&yaml_key()).expect("definition");
    *yaml = yaml.replace("EXACT_CONTENT", &"a".repeat(128 * 1024));
}

#[given("the other inline file is 131073 bytes")]
fn inline_file_over_limit(w: &mut BehaviourWorld) {
    let yaml = w.author_files.get_mut(&yaml_key()).expect("definition");
    *yaml = yaml.replace("OVERSIZED_CONTENT", &"b".repeat(128 * 1024 + 1));
}

#[given(regex = r#"^an lns\.yaml declaring two filesets mounted at "([^"]+)"$"#)]
fn lns_yaml_with_duplicate_filesets(w: &mut BehaviourWorld, mount: String) {
    seed(
        w,
        &fileset_yaml(&format!(
            "    - path: ./a\n      guestPath: {mount}\n    - path: ./b\n      guestPath: {mount}\n"
        )),
    );
}

#[given(
    regex = r#"^an lns\.yaml holding a connector declaring fileset "([^"]+)" mounted at "([^"]+)"$"#
)]
fn lns_yaml_holding_a_connector(w: &mut BehaviourWorld, path: String, mount: String) {
    seed(
        w,
        &format!(
            "apiVersion: lns.run/v1\nkind: connector\nname: some-provider\nspec:\n  serves:\n    - api.some-provider.example\n  methods:\n    - name: token\n      auth:\n        kind: token\n      credentials:\n        - envVar: SOME_TOKEN\n          placeholder: some_LNSPLACEHOLDER0000000000\n      filesets:\n        - path: {path}\n          guestPath: {mount}\n"
        ),
    );
}

#[given(regex = r#"^the project directory "([^"]+)" contains "([^"]+)" holding `([^`]*)`$"#)]
fn project_directory_contains_content(
    w: &mut BehaviourWorld,
    dir: String,
    file: String,
    content: String,
) {
    let path = PathBuf::from("/work")
        .join(dir.trim_start_matches("./"))
        .join(&file);
    w.author_files.insert(path, content);
}

#[given(regex = r#"^the project directory "([^"]+)" contains "([^"]+)"$"#)]
fn project_directory_contains(w: &mut BehaviourWorld, dir: String, file: String) {
    let path = PathBuf::from("/work")
        .join(dir.trim_start_matches("./"))
        .join(&file);
    w.author_files.insert(path, "fixture contents".to_string());
}

#[then(regex = r#"^a file "([^"]+)" is created$"#)]
fn file_created(w: &mut BehaviourWorld, name: String) -> Result<(), String> {
    if w.author_files
        .contains_key(&PathBuf::from("/work").join(&name))
    {
        Ok(())
    } else {
        Err(format!("{name} was not created"))
    }
}

#[then(regex = r#"^the file "([^"]+)" contains "([^"]+)"$"#)]
fn file_contains(w: &mut BehaviourWorld, name: String, needle: String) -> Result<(), String> {
    let contents = authored(w, &name)?;
    if contents.contains(&needle) {
        Ok(())
    } else {
        Err(format!(
            "expected {name} to contain {needle:?}, got:\n{contents}"
        ))
    }
}

#[then(regex = r#"^the file "([^"]+)" does not contain "([^"]+)"$"#)]
fn file_does_not_contain(
    w: &mut BehaviourWorld,
    name: String,
    needle: String,
) -> Result<(), String> {
    let contents = authored(w, &name)?;
    if contents.contains(&needle) {
        Err(format!(
            "expected {name} not to contain {needle:?}, got:\n{contents}"
        ))
    } else {
        Ok(())
    }
}

#[then(regex = r#"^the file "([^"]+)" does not contain "([^"]+)" in any casing$"#)]
fn file_does_not_contain_any_casing(
    w: &mut BehaviourWorld,
    name: String,
    needle: String,
) -> Result<(), String> {
    let contents = authored(w, &name)?;
    if contents.to_lowercase().contains(&needle.to_lowercase()) {
        Err(format!(
            "expected {name} not to contain {needle:?} in any casing, got:\n{contents}"
        ))
    } else {
        Ok(())
    }
}

fn authored<'a>(w: &'a BehaviourWorld, name: &str) -> Result<&'a String, String> {
    w.author_files
        .get(&PathBuf::from("/work").join(name))
        .ok_or_else(|| format!("{name} does not exist"))
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
