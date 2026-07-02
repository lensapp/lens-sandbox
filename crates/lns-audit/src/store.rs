use std::fs::File;
use std::io::{BufRead, BufReader, Lines};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use lns_ipc::LedgerRecord;
use serde_json::Value;

/// One ledger line: the reconstructed `record` for display, and the on-disk `raw` JSON (legacy or OCSF, `prev_hash` stripped) for pass-through emit.
#[derive(Debug)]
pub struct LedgerEntry {
    pub raw: Value,
    pub record: LedgerRecord,
}

/// Streams the ledger one entry per line so callers fold or filter without holding the whole file in memory; a missing ledger streams as empty.
pub fn stream_ledger(path: &Path) -> Result<LedgerStream> {
    let lines = match File::open(path) {
        Ok(file) => Some(BufReader::new(file).lines()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("reading ledger {}", path.display())),
    };
    Ok(LedgerStream {
        lines,
        path: path.to_path_buf(),
        line_no: 0,
    })
}

#[derive(Debug)]
pub struct LedgerStream {
    lines: Option<Lines<BufReader<File>>>,
    path: PathBuf,
    line_no: usize,
}

impl Iterator for LedgerStream {
    type Item = Result<LedgerEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        let lines = self.lines.as_mut()?;
        for line in lines.by_ref() {
            let line = match line {
                Ok(line) => line,
                Err(e) => {
                    let path = self.path.display();
                    return Some(Err(e).with_context(|| format!("reading ledger {path}")));
                }
            };
            self.line_no += 1;
            if line.trim().is_empty() {
                continue;
            }
            let line_no = self.line_no;
            let path = self.path.display();
            return Some(
                parse_ledger_line(&line)
                    .with_context(|| format!("parsing ledger line {line_no} of {path}")),
            );
        }
        None
    }
}

fn parse_ledger_line(line: &str) -> Result<LedgerEntry> {
    let Value::Object(mut obj) = serde_json::from_str::<Value>(line)? else {
        bail!("ledger line is not a JSON object");
    };
    let record = if crate::ocsf::is_ocsf(&obj) {
        crate::ocsf::ledger_record(&obj)?
    } else {
        serde_json::from_value::<LedgerRecord>(Value::Object(obj.clone()))?
    };
    obj.remove("prev_hash");
    Ok(LedgerEntry {
        raw: Value::Object(obj),
        record,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_ipc::{AuthKind, LedgerEvent};

    fn sample_line() -> String {
        let record = LedgerRecord {
            ts: "2026-06-29T14:05:30Z".into(),
            run: "5e6f7a8b0000000000000000000000bb".into(),
            microvm: "calm-finch".into(),
            event: LedgerEvent::CredentialUse {
                integration: "some-provider".into(),
                auth: AuthKind::Apikey,
                fp: Some("9c2f1a3d".into()),
                dest: vec!["api.some-provider.example".into()],
            },
        };
        serde_json::to_string(&record).unwrap()
    }

    fn collect(path: &Path) -> Result<Vec<LedgerEntry>> {
        stream_ledger(path)?.collect()
    }

    #[test]
    fn a_missing_ledger_streams_as_no_events_not_an_error() {
        let records = collect(Path::new("/nope/never/ledger.jsonl")).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn blank_lines_are_skipped_and_records_parse() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ledger.jsonl");
        let body = format!("{}\n\n{}\n", sample_line(), sample_line());
        std::fs::write(&path, body).unwrap();
        let records = collect(&path).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].record.run, "5e6f7a8b0000000000000000000000bb");
    }

    #[test]
    fn a_legacy_line_keeps_its_record_and_exposes_a_prev_hash_free_raw() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ledger.jsonl");
        let mut chain = lns_ipc::AuditChain::new();
        let augmented = chain.augment(&sample_line()).unwrap();
        std::fs::write(&path, augmented).unwrap();

        let entries = collect(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].record.event.name(), "credential_use");
        let raw = &entries[0].raw;
        assert_eq!(raw["event"], "credential_use");
        assert!(
            raw.get("prev_hash").is_none(),
            "raw leaks the chain link: {raw}"
        );
    }

    #[test]
    fn an_ocsf_line_reconstructs_the_record_and_keeps_the_ocsf_raw() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ledger.jsonl");
        let ev = lns_ocsf::credential_use(
            &lns_ocsf::Context {
                time_unix_secs: 1_780_000_000,
                ts_rfc3339: "2026-06-29T14:05:30Z",
                run: "5e6f7a8b0000000000000000000000bb",
                microvm: "calm-finch",
            },
            "some-provider",
            "apikey",
            Some("9c2f1a3d"),
            &["api.some-provider.example".into()],
        );
        let mut chain = lns_ipc::AuditChain::new();
        let augmented = chain.augment(&serde_json::to_string(&ev).unwrap()).unwrap();
        std::fs::write(&path, augmented).unwrap();

        let entries = collect(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].record.run, "5e6f7a8b0000000000000000000000bb");
        assert_eq!(entries[0].record.event.name(), "credential_use");
        assert_eq!(
            entries[0].raw["class_uid"], 3002,
            "the raw stays OCSF for pass-through emit"
        );
        assert!(entries[0].raw.get("prev_hash").is_none());
    }

    #[test]
    fn a_non_object_ledger_line_surfaces_a_parse_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ledger.jsonl");
        std::fs::write(&path, "42\n").unwrap();
        let err = collect(&path).unwrap_err();
        assert!(
            format!("{err:#}").contains("not a JSON object"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_malformed_line_surfaces_its_line_number() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ledger.jsonl");
        std::fs::write(&path, format!("{}\nnot-json\n", sample_line())).unwrap();
        let err = collect(&path).unwrap_err();
        assert!(format!("{err:#}").contains("line 2"), "got: {err:#}");
    }

    #[test]
    fn a_read_failure_mid_stream_surfaces_a_reading_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let err = collect(dir.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("reading ledger"),
            "got: {err:#}"
        );
    }

    #[test]
    fn an_unopenable_path_surfaces_a_reading_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let not_a_dir = dir.path().join("file");
        std::fs::write(&not_a_dir, "x").unwrap();
        let err = stream_ledger(&not_a_dir.join("child")).unwrap_err();
        assert!(
            format!("{err:#}").contains("reading ledger"),
            "got: {err:#}"
        );
    }
}
