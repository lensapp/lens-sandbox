use std::path::Path;

use anyhow::Result;
use lns_ipc::LedgerRecord;
use serde_json::{Map, Value};

use crate::oauth::Clock;

pub fn now_rfc3339(clock: &dyn Clock) -> String {
    crate::time_fmt::rfc3339_from_unix(clock.now_unix())
}

pub fn append_ledger_record(record: &LedgerRecord) -> Result<()> {
    let path = lns_ipc::connection_ledger()?;
    let anchor_path = lns_ipc::connection_ledger_anchor()?;
    append_ledger_record_at(&path, &anchor_path, record)
}

pub fn append_ledger_record_at(
    path: &Path,
    anchor_path: &Path,
    record: &LedgerRecord,
) -> Result<()> {
    let anchor = crate::audit::read_anchor(anchor_path);
    let mut sink = crate::audit::FileAnchorSink::new(anchor_path.to_path_buf());
    crate::audit::append_event_with_sink(path, anchor, record_to_object(record), &mut sink)
}

fn record_to_object(record: &LedgerRecord) -> Map<String, Value> {
    serde_json::to_value(record)
        .expect("a LedgerRecord serializes infallibly")
        .as_object()
        .cloned()
        .expect("a LedgerRecord serializes to a JSON object")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_ipc::{AuthKind, LedgerEvent};

    struct FakeClock(u64);
    impl Clock for FakeClock {
        fn now_unix(&self) -> u64 {
            self.0
        }
    }

    fn sample(run: u32) -> LedgerRecord {
        LedgerRecord {
            ts: "2026-06-29T14:02:11Z".into(),
            run,
            microvm: "calm-finch".into(),
            event: LedgerEvent::Connection {
                integration: "github".into(),
                auth: AuthKind::Oauth,
                account: Some("@hchen".into()),
                scopes: vec!["repo".into()],
                expires: None,
            },
        }
    }

    #[test]
    fn now_rfc3339_renders_the_clock_instant() {
        assert_eq!(now_rfc3339(&FakeClock(0)), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn append_writes_a_chained_record_and_anchor() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("ledger.jsonl");
        let anchor_path = d.path().join("ledger.anchor");
        append_ledger_record_at(&path, &anchor_path, &sample(49)).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"integration\":\"github\""), "{content}");
        assert!(
            content.contains(&format!("\"prev_hash\":\"{}\"", lns_ipc::GENESIS_PREV_HASH)),
            "first line must be genesis: {content}"
        );
        let back: LedgerRecord = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(back, sample(49));
        let anchor = crate::audit::read_anchor(&anchor_path).expect("anchor written");
        assert_eq!(anchor.line_count, 1);
    }

    #[test]
    fn successive_records_chain_and_advance_the_anchor() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("ledger.jsonl");
        let anchor_path = d.path().join("ledger.anchor");
        append_ledger_record_at(&path, &anchor_path, &sample(1)).unwrap();
        append_ledger_record_at(&path, &anchor_path, &sample(2)).unwrap();
        let anchor = crate::audit::read_anchor(&anchor_path).expect("anchor written");
        assert_eq!(anchor.line_count, 2);
    }

    #[test]
    #[serial_test::serial(env)]
    fn append_ledger_record_writes_under_data_root() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _x = crate::test_env::EnvVarGuard::set("XDG_DATA_HOME", d.path().join("data"));
        append_ledger_record(&sample(7)).unwrap();
        let content = std::fs::read_to_string(lns_ipc::connection_ledger().unwrap()).unwrap();
        assert!(content.contains("\"run\":7"), "{content}");
    }
}
