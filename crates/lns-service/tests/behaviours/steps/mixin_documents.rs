use cucumber::{given, then};

use crate::world::BehaviourWorld;

#[given(regex = r#"^the sandbox definition declares the mixin "([^"]+)"$"#)]
fn definition_declares_a_mixin(w: &mut BehaviourWorld, reference: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.definition = Some(format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"hermes"}},"spec":{{"image":"ghcr.io/team/base:1","mixins":["{reference}"]}}}}"#
    ));
}

#[then("the error says startup resolution is not implemented")]
fn error_says_resolution_is_not_implemented(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    let error = rig.error.as_deref().ok_or("no launch error was recorded")?;
    if error.contains("startup resolution is not implemented") {
        Ok(())
    } else {
        Err(format!(
            "a run that silently dropped a declared mixin would boot without what the document asked for; the refusal has to say why: {error}"
        ))
    }
}
