#[path = "behaviours/approval_rig.rs"]
mod approval_rig;
#[path = "behaviours/artifact_rig.rs"]
mod artifact_rig;
#[path = "behaviours/bind_rig.rs"]
mod bind_rig;
#[path = "behaviours/credential_rig.rs"]
mod credential_rig;
#[path = "behaviours/forward_rig.rs"]
mod forward_rig;
#[path = "behaviours/image_rig.rs"]
mod image_rig;
#[path = "behaviours/policy_rig.rs"]
mod policy_rig;
#[path = "behaviours/resolve_rig.rs"]
mod resolve_rig;
#[path = "behaviours/runner.rs"]
mod runner;
#[path = "behaviours/steps/mod.rs"]
mod steps;
#[path = "behaviours/volume_rig.rs"]
mod volume_rig;
#[path = "behaviours/world.rs"]
mod world;

use cucumber::World;
use std::path::PathBuf;
use world::BehaviourWorld;

#[tokio::main]
async fn main() {
    let features_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("behaviours");

    BehaviourWorld::cucumber()
        .fail_on_skipped()
        .filter_run_and_exit(features_dir, |feat, _, sc| {
            !feat.tags.iter().any(|t| t == "todo") && !sc.tags.iter().any(|t| t == "todo")
        })
        .await;
}
