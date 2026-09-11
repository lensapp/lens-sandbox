use crate::world::BehaviourWorld;
use cucumber::{given, then, when};

#[given(expr = "the CLI executable is {string}")]
fn executable(world: &mut BehaviourWorld, path: String) {
    world.installation_path = path.into();
}

#[when("loose-binary installation management is checked")]
fn check(world: &mut BehaviourWorld) {
    world.installation_error =
        lns_cli::installation::require_loose_binaries(&world.installation_path)
            .err()
            .map(|error| error.to_string());
}

#[then("installation management requires replacing or removing the complete app")]
fn refused(world: &mut BehaviourWorld) {
    let error = world
        .installation_error
        .as_ref()
        .expect("signed app helpers must not be managed as loose binaries");
    assert!(error.contains("complete app"), "{error}");
    assert!(error.contains("signature"), "{error}");
}

#[then("loose-binary installation management is allowed")]
fn allowed(world: &mut BehaviourWorld) {
    assert!(world.installation_error.is_none());
}
