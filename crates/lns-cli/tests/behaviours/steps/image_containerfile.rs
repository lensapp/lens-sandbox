use std::path::PathBuf;

use cucumber::given;

use crate::world::BehaviourWorld;

#[given(regex = r#"^an lns\.yaml whose image is "([^"]+)"$"#)]
fn lns_yaml_with_image(w: &mut BehaviourWorld, image: String) {
    w.author_files.insert(
        PathBuf::from("/work/lns.yaml"),
        format!(
            "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: \"{image}\"\n"
        ),
    );
}

#[given(regex = r#"^the directory "([^"]+)" holds a "([^"]+)"$"#)]
fn directory_holds(w: &mut BehaviourWorld, dir: String, file: String) {
    w.author_files.insert(
        PathBuf::from("/work").join(dir).join(file),
        "FROM docker.io/library/node:24-bookworm\n".to_string(),
    );
}
