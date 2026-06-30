use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use lns_ipc::{LedgerEvent, LedgerRecord};
use serde_json::{Map, Value};
use tokio::sync::mpsc;

use crate::oauth::Clock;

pub fn now_rfc3339(clock: &dyn Clock) -> String {
    crate::time_fmt::rfc3339_from_unix(clock.now_unix())
}

pub fn append_ledger_record(record: &LedgerRecord) -> Result<()> {
    // The machine-global ledger is written by every concurrent run; serialize so the hash chain can't interleave.
    static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = WRITE_LOCK.lock().expect("ledger write lock poisoned");
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

pub trait LedgerSink: Send + Sync {
    fn append(&self, record: &LedgerRecord) -> Result<()>;
}

pub struct FileLedgerSink;

impl LedgerSink for FileLedgerSink {
    fn append(&self, record: &LedgerRecord) -> Result<()> {
        append_ledger_record(record)
    }
}

/// Stamps a run's identity and the current time onto each event before persisting it; a write failure is logged, never propagated, so recording history can't break the run it describes.
pub trait LedgerRecorder: Send + Sync {
    fn record(&self, event: LedgerEvent);
}

pub struct RunLedgerRecorder {
    run: u32,
    microvm: String,
    clock: Arc<dyn Clock>,
    tx: Option<mpsc::UnboundedSender<LedgerRecord>>,
    writer: Option<std::thread::JoinHandle<()>>,
}

impl RunLedgerRecorder {
    pub fn new(run: u32, microvm: String, clock: Arc<dyn Clock>) -> Self {
        Self::with_sink(run, microvm, clock, Arc::new(FileLedgerSink))
    }

    pub fn with_sink(
        run: u32,
        microvm: String,
        clock: Arc<dyn Clock>,
        sink: Arc<dyn LedgerSink>,
    ) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<LedgerRecord>();
        let writer = std::thread::spawn(move || {
            while let Some(record) = rx.blocking_recv() {
                warn_on_record_failure(sink.append(&record));
            }
        });
        Self {
            run,
            microvm,
            clock,
            tx: Some(tx),
            writer: Some(writer),
        }
    }
}

impl LedgerRecorder for RunLedgerRecorder {
    fn record(&self, event: LedgerEvent) {
        let record = LedgerRecord {
            ts: now_rfc3339(&*self.clock),
            run: self.run,
            microvm: self.microvm.clone(),
            event,
        };
        if let Some(tx) = &self.tx {
            let _ = tx.send(record);
        }
    }
}

impl Drop for RunLedgerRecorder {
    /// Closing the channel then joining the writer flushes the run's queued events, so a single FIFO writer keeps them in record order and none are lost on teardown.
    fn drop(&mut self) {
        self.tx.take();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

fn warn_on_record_failure(result: Result<()>) {
    if let Err(e) = result {
        crate::log::warn!("could not record connection-ledger event: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_ipc::{ApprovalKind, AuthKind, Decision, LedgerEvent};

    struct FakeClock(u64);
    impl Clock for FakeClock {
        fn now_unix(&self) -> u64 {
            self.0
        }
    }

    #[derive(Default)]
    struct CapturingSink {
        records: std::sync::Mutex<Vec<LedgerRecord>>,
        fail: bool,
    }
    impl LedgerSink for CapturingSink {
        fn append(&self, record: &LedgerRecord) -> Result<()> {
            self.records.lock().unwrap().push(record.clone());
            if self.fail {
                anyhow::bail!("disk full");
            }
            Ok(())
        }
    }

    fn network_approval() -> LedgerEvent {
        LedgerEvent::Approval {
            kind: ApprovalKind::Network,
            target: "api.foo.com:443".into(),
            decision: Decision::AllowAlways,
            reason: None,
            integration: None,
        }
    }

    fn sample(run: u32) -> LedgerRecord {
        LedgerRecord {
            ts: "2026-06-29T14:02:11Z".into(),
            run,
            microvm: "calm-finch".into(),
            event: LedgerEvent::Connection {
                integration: "some-oauth".into(),
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
        assert!(
            content.contains("\"integration\":\"some-oauth\""),
            "{content}"
        );
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

    #[test]
    fn recorder_stamps_run_microvm_and_clock_onto_each_event() {
        let sink = Arc::new(CapturingSink::default());
        let recorder = RunLedgerRecorder::with_sink(
            49,
            "calm-finch".into(),
            Arc::new(FakeClock(0)),
            sink.clone(),
        );
        recorder.record(network_approval());
        drop(recorder);
        let records = sink.records.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].run, 49);
        assert_eq!(records[0].microvm, "calm-finch");
        assert_eq!(records[0].ts, "1970-01-01T00:00:00Z");
    }

    #[test]
    fn a_failing_sink_is_swallowed_so_recording_never_breaks_the_run() {
        let sink = Arc::new(CapturingSink {
            fail: true,
            ..Default::default()
        });
        let recorder =
            RunLedgerRecorder::with_sink(1, "vm".into(), Arc::new(FakeClock(0)), sink.clone());
        recorder.record(network_approval());
        drop(recorder);
        assert_eq!(
            sink.records.lock().unwrap().len(),
            1,
            "the append was attempted; its failure must not propagate"
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn the_file_recorder_persists_under_data_root() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _x = crate::test_env::EnvVarGuard::set("XDG_DATA_HOME", d.path().join("data"));
        let recorder = RunLedgerRecorder::new(7, "vm".into(), Arc::new(FakeClock(1_735_689_600)));
        recorder.record(LedgerEvent::Approval {
            kind: ApprovalKind::Network,
            target: "x".into(),
            decision: Decision::DenyOnce,
            reason: Some("policy-ambiguous".into()),
            integration: None,
        });
        drop(recorder);
        let content = std::fs::read_to_string(lns_ipc::connection_ledger().unwrap()).unwrap();
        assert!(content.contains("\"run\":7"), "{content}");
        assert!(content.contains("deny_once"), "{content}");
    }

    #[test]
    fn the_writer_preserves_record_order_even_when_an_early_append_is_slow() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Duration;

        #[derive(Default)]
        struct SlowFirstSink {
            targets: std::sync::Mutex<Vec<String>>,
            seen_first: AtomicBool,
        }
        impl LedgerSink for SlowFirstSink {
            fn append(&self, record: &LedgerRecord) -> Result<()> {
                if !self.seen_first.swap(true, Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                if let LedgerEvent::Approval { target, .. } = &record.event {
                    self.targets.lock().unwrap().push(target.clone());
                }
                Ok(())
            }
        }

        let sink = Arc::new(SlowFirstSink::default());
        let recorder =
            RunLedgerRecorder::with_sink(1, "vm".into(), Arc::new(FakeClock(0)), sink.clone());
        for target in ["a", "b", "c", "d", "e"] {
            recorder.record(LedgerEvent::Approval {
                kind: ApprovalKind::Network,
                target: target.into(),
                decision: Decision::AllowOnce,
                reason: None,
                integration: None,
            });
        }
        drop(recorder);
        assert_eq!(
            *sink.targets.lock().unwrap(),
            ["a", "b", "c", "d", "e"],
            "a single FIFO writer must persist events in record order even when an early append blocks; independent spawn_blocking tasks would let later events overtake"
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn concurrent_runs_appending_produce_one_unbroken_chain() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _x = crate::test_env::EnvVarGuard::set("XDG_DATA_HOME", d.path().join("data"));
        let threads: u32 = 8;
        let per_thread: u32 = 25;
        std::thread::scope(|scope| {
            for t in 0..threads {
                scope.spawn(move || {
                    for i in 0..per_thread {
                        append_ledger_record(&sample(t * per_thread + i)).unwrap();
                    }
                });
            }
        });

        let content = std::fs::read_to_string(lns_ipc::connection_ledger().unwrap()).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), (threads * per_thread) as usize);
        let mut expected_prev = lns_ipc::GENESIS_PREV_HASH.to_string();
        for line in &lines {
            let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(
                parsed["prev_hash"].as_str().unwrap(),
                expected_prev,
                "a concurrent append broke the chain order"
            );
            expected_prev = lns_ipc::line_hash(line.as_bytes());
        }
        let anchor =
            crate::audit::read_anchor(&lns_ipc::connection_ledger_anchor().unwrap()).unwrap();
        assert_eq!(anchor.line_count, (threads * per_thread) as u64);
    }
}
