#[path = "behaviours/runner.rs"]
mod runner;
#[path = "behaviours/steps/mod.rs"]
mod steps;
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

    // `@todo` scenarios are tracked in features but skipped here until the
    // step defs that implement them land; each scenario sheds the tag as it
    // goes green. Feature-level tags don't propagate to scenarios in
    // gherkin 0.x, so we check both.
    BehaviourWorld::cucumber()
        .fail_on_skipped()
        .filter_run_and_exit(features_dir, |feat, _, sc| {
            !feat.tags.iter().any(|t| t == "todo") && !sc.tags.iter().any(|t| t == "todo")
        })
        .await;
}
