use cucumber::{given, then, when};

use crate::steps::mixin_resolution::Installed;
use crate::world::BehaviourWorld;

fn mixin_document(name: &str, spec: &str) -> String {
    format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"Mixin","metadata":{{"name":"{name}"}},"spec":{spec}}}"#
    )
}

fn install_directory(w: &mut BehaviourWorld, dir: &str, spec: &str) {
    let name = dir.rsplit('/').next().unwrap_or("some-mixin").to_string();
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.mixins
        .insert(dir.to_string(), mixin_document(&name, spec));
}

#[given(regex = r#"^a mixin directory "([^"]+)" declaring the tool "([^"]+)"$"#)]
fn a_mixin_directory_declaring_a_tool(w: &mut BehaviourWorld, dir: String, tool: String) {
    install_directory(w, &dir, &format!(r#"{{"tools":["{tool}"]}}"#));
}

#[given(regex = r#"^a mixin directory "([^"]+)" declaring the mixin "([^"]+)"$"#)]
fn a_mixin_directory_declaring_a_mixin(w: &mut BehaviourWorld, dir: String, reference: String) {
    install_directory(w, &dir, &format!(r#"{{"mixins":["{reference}"]}}"#));
}

#[given(regex = r#"^the local definition at "([^"]+)" declares the mixin "([^"]+)"$"#)]
fn the_local_definition_declares_a_mixin(w: &mut BehaviourWorld, dir: String, reference: String) {
    local_definition(w, &dir, &format!(r#"["{reference}"]"#));
}

#[given(
    regex = r#"^the local definition at "([^"]+)" declares the mixins "([^"]+)" and "([^"]+)"$"#
)]
fn the_local_definition_declares_two_mixins(
    w: &mut BehaviourWorld,
    dir: String,
    first: String,
    second: String,
) {
    local_definition(w, &dir, &format!(r#"["{first}","{second}"]"#));
}

fn local_definition(w: &mut BehaviourWorld, dir: &str, mixins: &str) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.project_dir = Some(dir.to_string());
    rig.definition = Some(format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"hermes"}},"spec":{{"image":"ghcr.io/team/base:1","mixins":{mixins}}}}}"#
    ));
}

#[when("the local sandbox is resolved and launched")]
async fn the_local_sandbox_is_resolved_and_launched(w: &mut BehaviourWorld) {
    let (definition, home, installed) = {
        let rig = w.declared.get_or_insert_with(Default::default);
        (
            rig.definition
                .clone()
                .expect("a Given step must declare the definition"),
            rig.project_dir
                .clone()
                .expect("a Given step must place the definition"),
            Installed::from_rig(rig),
        )
    };
    let home = lns_service::artifact::mixin::Locator::Directory(std::path::PathBuf::from(home));
    let planned =
        match lns_service::artifact::mixin::resolve(definition.as_bytes(), &[], &home, &installed)
            .await
        {
            Ok(resolution) => {
                let rig = w.declared.get_or_insert_with(Default::default);
                rig.resolved_mixins.clone_from(&resolution.mixins);
                lns_service::artifact::plan_local_sandbox(&resolution.document)
            }
            Err(e) => Err(e),
        };
    crate::steps::declared_connectors::launch_resolved(w, planned);
}

#[then(regex = r#"^the resolution names the mixin "([^"]+)"$"#)]
fn the_resolution_names_the_mixin(w: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no resolution happened")?;
    if rig.resolved_mixins.contains(&expected) {
        Ok(())
    } else {
        Err(format!(
            "a directory has no digest, so the absolute path is the only thing the disclosure can name it by; got {:?}",
            rig.resolved_mixins
        ))
    }
}

#[then(regex = r#"^the resolution names only the mixin "([^"]+)"$"#)]
fn the_resolution_names_only_the_mixin(
    w: &mut BehaviourWorld,
    expected: String,
) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no resolution happened")?;
    if !rig.resolved_mixins.is_empty() && rig.resolved_mixins.iter().all(|m| *m == expected) {
        Ok(())
    } else {
        Err(format!(
            "two spellings of one directory are one identity, and a second identity would let it override itself; got {:?}",
            rig.resolved_mixins
        ))
    }
}

#[given(regex = r#"^the sandbox definition declares the mixin "([^"]+)"$"#)]
fn the_published_definition_declares_a_mixin(w: &mut BehaviourWorld, reference: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.definition = Some(format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"hermes"}},"spec":{{"image":"ghcr.io/team/base:1","mixins":["{reference}"]}}}}"#
    ));
}

#[then("the error says a published document cannot read a directory")]
fn the_error_says_a_published_document_cannot_read_a_directory(
    w: &mut BehaviourWorld,
) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    let error = rig.error.as_deref().ok_or("no launch error was recorded")?;
    if error.contains("a directory merges only into a document this machine read") {
        Ok(())
    } else {
        Err(format!(
            "a consumer has no copy of the author's working directory, so reading whatever path it names would merge something nobody published: {error}"
        ))
    }
}
