use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_artifact::spec::CredentialSlot;
use lns_policy::providers::is_self_identifying;
use lns_service::artifact::credential_boot::{
    Binding, BootGate, ConnectChoice, SlotOutcome, SlotPlan, boot_gate, plan_slot, resolve_connect,
};

fn declare(world: &mut BehaviourWorld, name: String, env: String, required: bool) {
    world.cred_boot().slot = Some(CredentialSlot {
        name,
        env,
        required,
    });
    world.cred_boot().bound_value = None;
}

#[given(
    regex = r#"^a bundle whose agent declares a credential slot for integration "([^"]+)" injected as "([^"]+)"$"#
)]
async fn declare_slot(world: &mut BehaviourWorld, name: String, env: String) {
    declare(world, name, env, false);
}

#[given(regex = r#"^an unbound credential slot for integration "([^"]+)" injected as "([^"]+)"$"#)]
async fn unbound_slot(world: &mut BehaviourWorld, name: String, env: String) {
    declare(world, name, env, false);
}

#[given(
    regex = r#"^an unbound required credential slot for integration "([^"]+)" injected as "([^"]+)"$"#
)]
async fn unbound_required_slot(world: &mut BehaviourWorld, name: String, env: String) {
    declare(world, name, env, true);
}

#[given(
    regex = r#"^an unbound optional credential slot for integration "([^"]+)" injected as "([^"]+)"$"#
)]
async fn unbound_optional_slot(world: &mut BehaviourWorld, name: String, env: String) {
    declare(world, name, env, false);
}

#[given(regex = r#"^the per-machine credential store has a bound value for "([^"]+)"$"#)]
async fn store_has_binding(world: &mut BehaviourWorld, name: String) {
    let rig = world.cred_boot();
    rig.bound_value = Some(format!("real-secret-for-{name}"));
    rig.placeholder = Some(format!("{name}_LNSPLACEHOLDER0000000000000000000000"));
}

#[given(regex = r#"^the per-machine credential store has no value for "([^"]+)"$"#)]
async fn store_has_no_binding(world: &mut BehaviourWorld, _name: String) {
    world.cred_boot().bound_value = None;
}

fn form_plan(world: &mut BehaviourWorld) {
    let rig = world.cred_boot();
    let slot = rig.slot.clone().expect("a scenario must declare a slot");
    let binding = rig.bound_value.as_ref().map(|_| Binding {
        placeholder: rig
            .placeholder
            .clone()
            .expect("a bound slot must carry a placeholder"),
    });
    rig.plan = Some(plan_slot(&slot, binding));
}

#[when("the bundle is launched")]
async fn bundle_launched(world: &mut BehaviourWorld) {
    form_plan(world);
}

#[when("the connect prompt is shown")]
async fn connect_prompt_shown(world: &mut BehaviourWorld) {
    form_plan(world);
}

#[when(regex = r#"^the developer declines to connect "([^"]+)"$"#)]
async fn developer_declines(world: &mut BehaviourWorld, _name: String) {
    form_plan(world);
    let prompt = match world.cred_boot().plan.clone() {
        Some(SlotPlan::Connect(prompt)) => prompt,
        other => panic!("declining requires a pending connect prompt, got {other:?}"),
    };
    world.cred_boot().outcome = Some(resolve_connect(&prompt, ConnectChoice::Decline));
}

fn plan(world: &mut BehaviourWorld) -> SlotPlan {
    world
        .cred_boot()
        .plan
        .clone()
        .expect("a step must have formed the slot plan")
}

fn connect_prompt(
    world: &mut BehaviourWorld,
) -> lns_service::artifact::credential_boot::ConnectPrompt {
    match plan(world) {
        SlotPlan::Connect(prompt) => prompt,
        other => panic!("expected a connect prompt, got {other:?}"),
    }
}

fn outcome(world: &mut BehaviourWorld) -> SlotOutcome {
    world
        .cred_boot()
        .outcome
        .expect("a step must have resolved the connect decision")
}

#[then("the slot is resolved from the store at boot")]
async fn resolved_from_store(world: &mut BehaviourWorld) {
    assert!(
        matches!(plan(world), SlotPlan::Armed { .. }),
        "a bound slot must arm from the store without a prompt",
    );
}

#[then("no credential prompt is shown")]
async fn no_prompt(world: &mut BehaviourWorld) {
    assert!(!matches!(plan(world), SlotPlan::Connect(_)));
}

#[then("the workload starts")]
async fn workload_starts(world: &mut BehaviourWorld) {
    let plan = plan(world);
    assert_eq!(
        boot_gate(std::slice::from_ref(&plan)),
        BootGate::StartWorkload
    );
}

#[then(regex = r#"^a connect prompt for "([^"]+)" is shown before the workload starts$"#)]
async fn prompt_before_start(world: &mut BehaviourWorld, name: String) {
    let prompt = connect_prompt(world);
    assert_eq!(prompt.integration, name);
    let plan = plan(world);
    assert_eq!(
        boot_gate(std::slice::from_ref(&plan)),
        BootGate::AwaitConnect
    );
}

#[then("the workload does not start until the slot is decided")]
async fn gated_until_decided(world: &mut BehaviourWorld) {
    let plan = plan(world);
    assert_eq!(
        boot_gate(std::slice::from_ref(&plan)),
        BootGate::AwaitConnect
    );
    assert!(
        world.cred_boot().outcome.is_none(),
        "no decision should have been recorded yet",
    );
}

#[then(
    regex = r#"^the prompt names the injection target "([^"]+)" before any real value is entered$"#
)]
async fn prompt_names_target(world: &mut BehaviourWorld, env: String) {
    assert_eq!(connect_prompt(world).env, env);
    assert!(
        world.cred_boot().bound_value.is_none(),
        "no real value should exist when the prompt is first shown",
    );
}

#[then("the launch is aborted")]
async fn launch_aborted(world: &mut BehaviourWorld) {
    assert_eq!(outcome(world), SlotOutcome::AbortLaunch);
}

#[then("the workload never starts")]
async fn workload_never_starts(world: &mut BehaviourWorld) {
    assert!(!outcome(world).starts_workload());
}

#[then(regex = r#"^the workload starts with "([^"]+)" left unbound$"#)]
async fn starts_with_unbound(world: &mut BehaviourWorld, env: String) {
    assert_eq!(outcome(world), SlotOutcome::LeftUnbound);
    assert!(outcome(world).starts_workload());
    assert_eq!(
        world.cred_boot().slot.as_ref().expect("slot").env,
        env,
        "the unbound slot names the disclosed injection target",
    );
}

#[then(regex = r#"^the workload sees only a placeholder in "([^"]+)"$"#)]
async fn workload_sees_placeholder(world: &mut BehaviourWorld, env: String) {
    let (plan_env, placeholder) = match plan(world) {
        SlotPlan::Armed { env, placeholder } => (env, placeholder),
        other => panic!("expected an armed slot, got {other:?}"),
    };
    assert_eq!(plan_env, env);
    assert!(
        is_self_identifying(&placeholder),
        "the injected value must be a self-identifying placeholder, got {placeholder}",
    );
    let real = world.cred_boot().bound_value.clone().expect("bound value");
    assert_ne!(
        placeholder, real,
        "the real secret must never reach the env"
    );
}

#[then("the real value is substituted at the boundary")]
async fn value_at_boundary(world: &mut BehaviourWorld) {
    let placeholder = match plan(world) {
        SlotPlan::Armed { placeholder, .. } => placeholder,
        other => panic!("expected an armed slot, got {other:?}"),
    };
    let real = world.cred_boot().bound_value.clone().expect("bound value");
    assert!(
        !placeholder.contains(&real),
        "the plan handed to the workload must carry only the placeholder; \
         the real value stays at the boundary",
    );
}
