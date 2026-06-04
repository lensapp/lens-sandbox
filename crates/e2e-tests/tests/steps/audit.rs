use crate::E2eWorld;
use crate::specutil::{TAMPERED_AUDIT_CHAIN, VALID_AUDIT_CHAIN, cache_runs_dir};
use cucumber::given;
use std::fs;

#[given(regex = r#"^a run "([^"]+)" with a valid audit chain$"#)]
fn valid_chain(world: &mut E2eWorld, run_id: String) {
    write_fixture(world, &run_id, VALID_AUDIT_CHAIN);
}

#[given(regex = r#"^a run "([^"]+)" with a tampered audit chain$"#)]
fn tampered_chain(world: &mut E2eWorld, run_id: String) {
    write_fixture(world, &run_id, TAMPERED_AUDIT_CHAIN);
}

fn write_fixture(world: &E2eWorld, run_id: &str, contents: &str) {
    let home = world
        .home
        .as_ref()
        .expect("Given a clean lns cache home before writing a fixture");
    let run_dir = cache_runs_dir(home.path()).join(run_id);
    fs::create_dir_all(&run_dir).expect("create run dir");
    fs::write(run_dir.join("audit.jsonl"), contents).expect("write audit.jsonl");
}
