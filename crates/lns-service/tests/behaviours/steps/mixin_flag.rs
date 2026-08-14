use cucumber::{given, then, when};

use crate::steps::mixin_resolution::{Installed, MIXIN, install_at, published};
use crate::world::BehaviourWorld;

/// What a tag the user names resolves to, since a registry answers for one under exact bytes.
const PINNED_TAG: &str = "ghcr.io/acme/obs-tools@sha256:5b9e1f0a7c3d284e6b15f907a2c8d63b40e19a7c25f8b0d3e6a94c17f582aa41";

#[given(regex = r#"^a mixin declaring the tool "([^"]+)" published under the tag "([^"]+)"$"#)]
fn a_mixin_published_under_a_tag(w: &mut BehaviourWorld, tool: String, tag: String) {
    install_at(w, &tag, "obs-tools", &format!(r#"{{"tools":["{tool}"]}}"#));
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.mixin_pins.insert(tag, PINNED_TAG.to_string());
}

#[given(regex = r#"^the sandbox definition declares the tool "([^"]+)"$"#)]
fn the_definition_declares_a_tool(w: &mut BehaviourWorld, tool: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.definition = Some(format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{{"image":"ghcr.io/team/base:1","tools":["{tool}"]}}}}"#
    ));
}

#[when("the published sandbox is resolved with that mixin added by the user")]
async fn resolved_with_that_mixin_added(w: &mut BehaviourWorld) {
    resolve_with(w, &[MIXIN.to_string()]).await;
}

#[when(regex = r#"^the published sandbox is resolved with the user's mixin "([^"]+)"$"#)]
async fn resolved_with_the_users_mixin(w: &mut BehaviourWorld, reference: String) {
    resolve_with(w, &[reference]).await;
}

async fn resolve_with(w: &mut BehaviourWorld, extra: &[String]) {
    let (definition, installed) = {
        let rig = w.declared.get_or_insert_with(Default::default);
        let definition = rig.definition.clone().unwrap_or_else(|| {
            r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1"}}"#
                .to_string()
        });
        (definition, Installed::from_rig(rig))
    };
    let planned = match lns_service::artifact::mixin::resolve(
        definition.as_bytes(),
        extra,
        &published(),
        &installed,
    )
    .await
    {
        Ok(resolution) => {
            let rig = w.declared.get_or_insert_with(Default::default);
            rig.resolved_mixins.clone_from(&resolution.mixins);
            rig.pinned_extra.clone_from(&resolution.pinned_extra);
            lns_service::artifact::plan_published_sandbox(
                &resolution.document,
                "registry.example.test/some-sandbox:1",
            )
        }
        Err(e) => Err(e),
    };
    crate::steps::declared_connectors::launch_resolved(w, planned);
}

#[then("the resolution reports the mixin pinned by digest")]
fn resolution_reports_a_pinned_mixin(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no resolution happened")?;
    if rig.resolved_mixins.is_empty() {
        return Err("the resolution named no mixin at all".to_string());
    }
    match rig
        .resolved_mixins
        .iter()
        .find(|entry| !entry.contains("@sha256:"))
    {
        None => Ok(()),
        Some(unpinned) => Err(format!(
            "a disclosure that names a tag lets the bytes it stood for change after the user approved them: {unpinned}"
        )),
    }
}

#[then(regex = r#"^the resolution answers for the tag "([^"]+)"$"#)]
fn resolution_answers_for_the_tag(w: &mut BehaviourWorld, tag: String) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no resolution happened")?;
    match rig.pinned_extra.as_slice() {
        [pinned] if pinned.contains("@sha256:") && *pinned != tag => Ok(()),
        other => Err(format!(
            "the boot has to receive the pin the preflight showed, one per reference the user named, or the tag can move between them; got {other:?}"
        )),
    }
}

#[then("the error says a directory merges only into a document this machine read")]
fn error_says_a_directory_needs_a_document_this_machine_read(
    w: &mut BehaviourWorld,
) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    let error = rig.error.as_deref().ok_or("no launch error was recorded")?;
    if error.contains("a directory merges only into a document this machine read") {
        Ok(())
    } else {
        Err(format!(
            "a published sandbox's filesets are pinned by digest, so a working directory merged into one would boot bytes the disclosure cannot account for: {error}"
        ))
    }
}
