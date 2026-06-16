use crate::E2eWorld;
use crate::specutil::{TAMPERED_AUDIT_CHAIN, cache_runs_dir};
use cucumber::given;
use std::fs;
use std::path::PathBuf;

#[given(regex = r#"^a run "([^"]+)" with a valid audit chain$"#)]
fn valid_chain(world: &mut E2eWorld, run_id: String) {
    let run_dir = run_dir(world, &run_id);
    let mut chain = lns_ipc::AuditChain::new();
    let mut payload = String::new();
    for event in [
        r#"{"type":"audit_event","seq":1}"#,
        r#"{"type":"audit_event","seq":2}"#,
    ] {
        let augmented = chain.augment(event).expect("augment audit event");
        payload.push_str(std::str::from_utf8(&augmented).expect("augmented line is utf8"));
        payload.push('\n');
    }
    fs::write(run_dir.join("audit.jsonl"), payload).expect("write audit.jsonl");
    let anchor = chain.anchor().expect("a non-empty chain anchors its head");
    fs::write(run_dir.join("audit.anchor"), anchor.to_line()).expect("write audit.anchor");
}

#[given(regex = r#"^a run "([^"]+)" with a tampered audit chain$"#)]
fn tampered_chain(world: &mut E2eWorld, run_id: String) {
    let run_dir = run_dir(world, &run_id);
    fs::write(run_dir.join("audit.jsonl"), TAMPERED_AUDIT_CHAIN).expect("write audit.jsonl");
}

fn run_dir(world: &E2eWorld, run_id: &str) -> PathBuf {
    let home = world
        .home
        .as_ref()
        .expect("Given a clean lns cache home before writing a fixture");
    let dir = cache_runs_dir(home.path()).join(run_id);
    fs::create_dir_all(&dir).expect("create run dir");
    dir
}
