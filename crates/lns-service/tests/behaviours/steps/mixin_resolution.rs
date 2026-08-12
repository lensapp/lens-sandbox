use cucumber::{given, then, when};
use lns_service::artifact::mixin::MixinSource;

use crate::world::BehaviourWorld;

/// The one mixin every scenario declares, digest-pinned because a published document may reference nothing else.
const MIXIN: &str = "ghcr.io/acme/some-mixin@sha256:c41e8b7d20a95f6c3d84b1e07f92a5c8d63b40e19a7c25f8b0d3e6a94c17f582";

/// Serves the mixin documents a scenario installed, standing in for the registry the run pulls from.
struct Installed(std::collections::BTreeMap<String, String>);

impl MixinSource for Installed {
    async fn fetch(&self, reference: &str) -> anyhow::Result<String> {
        self.0
            .get(reference)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("nothing here answers for {reference}"))
    }
}

fn install(w: &mut BehaviourWorld, spec: &str) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.mixins.insert(
        MIXIN.to_string(),
        format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"Mixin","metadata":{{"name":"some-mixin"}},"spec":{spec}}}"#
        ),
    );
}

fn definition(w: &mut BehaviourWorld, spec: &str) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.definition = Some(format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"hermes"}},"spec":{spec}}}"#
    ));
}

#[given(regex = r#"^a mixin declaring the tool "([^"]+)"$"#)]
fn mixin_declaring_a_tool(w: &mut BehaviourWorld, tool: String) {
    install(w, &format!(r#"{{"tools":["{tool}"]}}"#));
}

#[given(regex = r#"^a mixin allowing "([^"]+)"$"#)]
fn mixin_allowing_a_destination(w: &mut BehaviourWorld, host: String) {
    install(
        w,
        &format!(
            r#"{{"policy":{{"egress":{{"http":[{{"match":"{host}","verdict":"allow"}}]}}}}}}"#
        ),
    );
}

#[given(regex = r#"^a mixin declaring the credential "([^"]+)" for "([^"]+)"$"#)]
fn mixin_declaring_a_credential(w: &mut BehaviourWorld, env_var: String, domain: String) {
    install(
        w,
        &format!(
            r#"{{"credentials":[{{"envVar":"{env_var}","placeholder":"lns-placeholder-{env_var}","injections":[{{"kind":"bearer_header","domain":"{domain}"}}]}}]}}"#
        ),
    );
}

#[given("the sandbox definition declares that mixin")]
fn definition_declares_that_mixin(w: &mut BehaviourWorld) {
    definition(
        w,
        &format!(r#"{{"image":"ghcr.io/team/base:1","mixins":["{MIXIN}"]}}"#),
    );
}

#[given(regex = r#"^the sandbox definition declares the tool "([^"]+)" and that mixin$"#)]
fn definition_declares_a_tool_and_that_mixin(w: &mut BehaviourWorld, tool: String) {
    definition(
        w,
        &format!(r#"{{"image":"ghcr.io/team/base:1","tools":["{tool}"],"mixins":["{MIXIN}"]}}"#),
    );
}

#[given("the sandbox definition declares a mixin nothing can resolve")]
fn definition_declares_an_unresolvable_mixin(w: &mut BehaviourWorld) {
    definition(
        w,
        &format!(r#"{{"image":"ghcr.io/team/base:1","mixins":["{MIXIN}"]}}"#),
    );
}

#[when("the published sandbox is resolved and launched")]
async fn sandbox_is_resolved_and_launched(w: &mut BehaviourWorld) {
    let (definition, installed) = {
        let rig = w.declared.get_or_insert_with(Default::default);
        (
            rig.definition
                .clone()
                .expect("a Given step must declare the definition"),
            Installed(rig.mixins.clone()),
        )
    };
    let planned =
        match lns_service::artifact::mixin::resolve(definition.as_bytes(), &installed).await {
            Ok(resolution) => lns_service::artifact::plan_published_sandbox(
                &resolution.document,
                "registry.example.test/some-sandbox:1",
            ),
            Err(e) => Err(e),
        };
    crate::steps::declared_connectors::launch_resolved(w, planned);
}

#[then(regex = r#"^the run installs "([^"]+)"$"#)]
fn run_installs(w: &mut BehaviourWorld, tool: String) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    if let Some(err) = &rig.error {
        return Err(format!("the launch failed: {err}"));
    }
    if rig.tools.contains(&tool) {
        Ok(())
    } else {
        Err(format!(
            "expected the resolved sandbox to ask for {tool}, got {:?}",
            rig.tools
        ))
    }
}

#[then(regex = r#"^a workload request to "([^"]+)" is allowed by policy$"#)]
fn request_allowed_by_policy(w: &mut BehaviourWorld, host: String) -> Result<(), String> {
    let policy = w
        .declared
        .as_ref()
        .and_then(|r| r.running_policy.as_ref())
        .ok_or("no running policy was produced")?;
    let verdict = policy
        .network
        .egress
        .http
        .iter()
        .find(|r| r.match_pattern == host)
        .map(|r| r.verdict);
    if verdict == Some(lns_policy::Verdict::Allow) {
        Ok(())
    } else {
        Err(format!(
            "expected {host} to be allowed by what the mixin contributed, got {verdict:?}"
        ))
    }
}

#[then("the error says the mixin could not be resolved")]
fn error_says_unresolvable(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    let error = rig.error.as_deref().ok_or("no launch error was recorded")?;
    if error.contains("resolving mixin") {
        Ok(())
    } else {
        Err(format!(
            "a run that dropped a mixin it could not fetch would boot without what its document asked for: {error}"
        ))
    }
}

#[then("the error says a local document's mixins are not resolved yet")]
fn error_says_local_mixins_are_unresolved(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    let error = rig.error.as_deref().ok_or("no launch error was recorded")?;
    if error.contains("a local document's mixins are not resolved yet") {
        Ok(())
    } else {
        Err(format!(
            "a local run's mounts and ports come from the document the CLI parsed itself, so booting one that declares mixins would drop what they contribute without a word: {error}"
        ))
    }
}
