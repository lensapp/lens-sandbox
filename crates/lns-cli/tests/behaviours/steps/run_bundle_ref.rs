use crate::world::BehaviourWorld;
use cucumber::{given, then};
use lns_cli::registry::RegistryClient;
use lns_policy::artifact::Family;

async fn store(world: &mut BehaviourWorld, reference: &str, family: Family, yaml: &str) {
    let blob = lns_policy::artifact::to_config_blob(yaml.as_bytes()).expect("artifact yaml");
    world
        .registry
        .push_artifact(
            reference,
            &family.artifact_type(),
            &family.config_media_type(),
            &blob,
        )
        .await
        .expect("store artifact");
}

#[given(regex = r#"^a bundle "([^"]+)" with agent image "([^"]+)" and an egress policy$"#)]
async fn given_bundle_with_policy(world: &mut BehaviourWorld, bundle_ref: String, image: String) {
    store(
        world,
        "localhost:5000/org/acme/agents/some-agent:v1",
        Family::Agent,
        &format!(
            "apiVersion: lens.dev/v1alpha1\nkind: Agent\n\
             metadata:\n  name: some-agent\nspec:\n  image: {image}\n"
        ),
    )
    .await;
    store(
        world,
        "localhost:5000/org/acme/policies/some-egress:v1",
        Family::Policy,
        "network:\n  defaultVerdict: ask\n",
    )
    .await;
    store(
        world,
        &bundle_ref,
        Family::Bundle,
        "apiVersion: lens.dev/v1alpha1\nkind: AgentSystem\n\
         metadata:\n  name: some-system\n\
         spec:\n  components:\n    \
         agents:\n      - { ref: org/acme/agents/some-agent:v1 }\n    \
         policies:\n      - { ref: org/acme/policies/some-egress:v1 }\n",
    )
    .await;
}

#[then("the run uses an ephemeral policy outside the project directory")]
fn then_ephemeral_policy(world: &mut BehaviourWorld) -> Result<(), String> {
    let policy = world
        .resolved_policy
        .as_ref()
        .ok_or("expected a materialized policy path")?;
    if !policy.exists() {
        return Err(format!("ephemeral policy {policy:?} does not exist"));
    }
    if policy.ends_with("lns-policy.yaml") {
        return Err(format!("policy must not be the project file: {policy:?}"));
    }
    Ok(())
}

#[then("no policy file is created in the project directory")]
fn then_cwd_untouched(world: &mut BehaviourWorld) -> Result<(), String> {
    if let Some(dir) = &world.cwd {
        let cwd_policy = dir.path().join("lns-policy.yaml");
        if cwd_policy.exists() {
            return Err(format!(
                "the project directory must stay untouched: {cwd_policy:?}"
            ));
        }
    }
    Ok(())
}
