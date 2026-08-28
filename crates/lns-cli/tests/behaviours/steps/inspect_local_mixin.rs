use std::path::PathBuf;

use cucumber::given;

use crate::world::BehaviourWorld;

fn document(directory: &str) -> PathBuf {
    PathBuf::from("/work")
        .join(directory.trim_start_matches("./"))
        .join("lns.yaml")
}

fn sandbox_layering_on(entries: &str) -> String {
    format!(
        "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n{entries}"
    )
}

fn declaring(reference: &str) -> String {
    sandbox_layering_on(&format!("  mixins:\n    - {reference}\n"))
}

#[given(regex = r#"^an lns\.yaml layering on "([^"]+)"$"#)]
fn lns_yaml_layering_on(w: &mut BehaviourWorld, reference: String) {
    w.author_files
        .insert(PathBuf::from("/work/lns.yaml"), declaring(&reference));
}

#[given(regex = r#"^the definition file "([^"]+)" layers on "([^"]+)"$"#)]
fn definition_file_layering_on(w: &mut BehaviourWorld, file: String, reference: String) {
    w.author_files
        .insert(PathBuf::from("/work").join(file), declaring(&reference));
}

#[given(regex = r#"^the definition file "([^"]+)" layers on nothing$"#)]
fn definition_file_layering_on_nothing(w: &mut BehaviourWorld, file: String) {
    w.author_files
        .insert(PathBuf::from("/work").join(file), sandbox_layering_on(""));
}

#[given(regex = r#"^the mixin "([^"]+)" declares tool "([^"]+)"$"#)]
fn mixin_declaring_a_tool(w: &mut BehaviourWorld, directory: String, tool: String) {
    let name = directory.trim_start_matches("./").replace('/', "-");
    w.author_files.insert(
        document(&directory),
        format!(
            "apiVersion: lns.run/v1\nkind: mixin\nname: {name}\nspec:\n  tools:\n    - {tool}\n"
        ),
    );
}

#[given(regex = r#"^the mixin "([^"]+)" also declares tool "([^"]+)"$"#)]
fn mixin_declaring_another_tool(w: &mut BehaviourWorld, directory: String, tool: String) {
    let path = document(&directory);
    let existing = w
        .author_files
        .get(&path)
        .expect("the mixin has to exist before it declares another tool")
        .clone();
    w.author_files
        .insert(path, format!("{existing}    - {tool}\n"));
}

#[given(regex = r#"^the mixin "([^"]+)" layers on "([^"]+)"$"#)]
fn mixin_layering_on_another(w: &mut BehaviourWorld, directory: String, reference: String) {
    let path = document(&directory);
    let existing = w
        .author_files
        .get(&path)
        .expect("the mixin has to exist before it layers on another")
        .clone();
    w.author_files
        .insert(path, format!("{existing}  mixins:\n    - {reference}\n"));
}

#[given(regex = r#"^the mixin "([^"]+)" holds a sandbox document$"#)]
fn mixin_path_holding_a_sandbox(w: &mut BehaviourWorld, directory: String) {
    w.author_files.insert(
        document(&directory),
        "apiVersion: lns.run/v1\nkind: sandbox\nname: obs\nspec:\n  image: ghcr.io/team/base:1\n"
            .to_string(),
    );
}

#[given(regex = r#"^the mixin "([^"]+)" holds malformed yaml$"#)]
fn mixin_holding_malformed_yaml(w: &mut BehaviourWorld, directory: String) {
    w.author_files
        .insert(document(&directory), "spec: [unterminated".to_string());
}
