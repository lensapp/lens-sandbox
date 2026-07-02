use crate::E2eWorld;
use crate::specutil::{audit_runs_dir, data_lns_dir};
use cucumber::given;
use std::fs;
use std::path::{Path, PathBuf};

fn octx<'a>(run: &'a str, ts: &'a str) -> lns_ocsf::Context<'a> {
    lns_ocsf::Context {
        time_unix_secs: 1_782_741_720,
        ts_rfc3339: ts,
        run,
        microvm: "calm-finch",
    }
}

fn write_chained(dir: &Path, log: &str, anchor: &str, lines: &[String]) {
    let mut chain = lns_ipc::AuditChain::new();
    let mut payload = String::new();
    for line in lines {
        let augmented = chain.augment(line).expect("augment audit line");
        payload.push_str(std::str::from_utf8(&augmented).expect("augmented line is utf8"));
        payload.push('\n');
    }
    fs::write(dir.join(log), payload).expect("write log");
    fs::write(dir.join(anchor), chain.anchor().unwrap().to_line()).expect("write anchor");
}

fn connection_ledger_events() -> [String; 2] {
    [
        lns_ocsf::connection(
            &octx("41aaaaaa000000000000000000000000", "2026-06-29T14:02:11Z"),
            "some-oauth",
            "oauth",
            Some("@some-user"),
            &["repo".to_string(), "read:org".to_string()],
            None,
        )
        .to_string(),
        lns_ocsf::credential_use(
            &octx("49bbbbbb000000000000000000000000", "2026-06-29T14:05:30Z"),
            "some-provider",
            "apikey",
            Some("9c2f1a3d"),
            &["api.some-provider.example".to_string()],
        )
        .to_string(),
    ]
}

#[given(regex = r#"^a run "([^"]+)" with a valid audit chain$"#)]
fn valid_chain(world: &mut E2eWorld, run_id: String) {
    let dir = run_dir(world, &run_id);
    let events = [
        lns_ocsf::volume_mount(&octx(&run_id, "2026-06-29T14:00:00Z"), "data", "/data").to_string(),
        lns_ocsf::volume_mount(&octx(&run_id, "2026-06-29T14:01:00Z"), "cache", "/cache")
            .to_string(),
    ];
    write_chained(&dir, "audit.jsonl", "audit.anchor", &events);
}

#[given(regex = r#"^a run "([^"]+)" with a tampered audit chain$"#)]
fn tampered_chain(world: &mut E2eWorld, run_id: String) {
    let dir = run_dir(world, &run_id);
    let mut event = lns_ocsf::volume_mount(&octx(&run_id, "2026-06-29T14:00:00Z"), "data", "/data");
    event
        .as_object_mut()
        .unwrap()
        .insert("prev_hash".into(), "deadbeef".into());
    fs::write(dir.join("audit.jsonl"), format!("{event}\n")).expect("write audit.jsonl");
}

#[given(regex = r#"^a run "([^"]+)" with a guest egress event$"#)]
fn guest_egress(world: &mut E2eWorld, run_id: String) {
    let dir = run_dir(world, &run_id);
    let event = lns_ocsf::egress(
        &octx(&run_id, "2026-06-29T14:02:00Z"),
        "GET",
        "http://api.example.test:443/",
        Some(200),
        Some("success"),
        Some("user-allowed-once"),
        true,
    )
    .to_string();
    write_chained(&dir, "audit.jsonl", "audit.anchor", &[event]);
}

#[given("a connection ledger with sample events")]
fn connection_ledger(world: &mut E2eWorld) {
    write_chained(
        &ledger_dir(world),
        "ledger.jsonl",
        "ledger.anchor",
        &connection_ledger_events(),
    );
}

#[given("a connection ledger with a tampered event")]
fn tampered_connection_ledger(world: &mut E2eWorld) {
    let dir = ledger_dir(world);
    write_chained(
        &dir,
        "ledger.jsonl",
        "ledger.anchor",
        &connection_ledger_events(),
    );
    let good = fs::read_to_string(dir.join("ledger.jsonl")).unwrap();
    fs::write(
        dir.join("ledger.jsonl"),
        good.replacen("calm-finch", "tampered", 1),
    )
    .expect("write ledger.jsonl");
}

fn run_dir(world: &E2eWorld, run_id: &str) -> PathBuf {
    let home = world
        .home
        .as_ref()
        .expect("Given a clean lns cache home before writing a fixture");
    let dir = audit_runs_dir(home.path()).join(run_id);
    fs::create_dir_all(&dir).expect("create run dir");
    dir
}

fn ledger_dir(world: &E2eWorld) -> PathBuf {
    let home = world
        .home
        .as_ref()
        .expect("Given a clean lns cache home before writing a fixture");
    let dir = data_lns_dir(home.path());
    fs::create_dir_all(&dir).expect("create data dir");
    dir
}
