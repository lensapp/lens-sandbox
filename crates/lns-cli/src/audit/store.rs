use std::path::Path;

use anyhow::{Context, Result};
use lns_ipc::LedgerRecord;

pub fn read_ledger(path: &Path) -> Result<Vec<LedgerRecord>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(e).with_context(|| format!("reading ledger {}", path.display()));
        }
    };
    parse_ledger(&text, path)
}

pub fn parse_ledger(text: &str, path: &Path) -> Result<Vec<LedgerRecord>> {
    let mut records = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: LedgerRecord = serde_json::from_str(line)
            .with_context(|| format!("parsing ledger line {} of {}", idx + 1, path.display()))?;
        records.push(record);
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_ipc::{AuthKind, LedgerEvent};

    fn sample_line() -> String {
        let record = LedgerRecord {
            ts: "2026-06-29T14:05:30Z".into(),
            run: 49,
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

    #[test]
    fn a_missing_ledger_reads_as_no_events_not_an_error() {
        let records = read_ledger(Path::new("/nope/never/ledger.jsonl")).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn blank_lines_are_skipped_and_records_parse() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ledger.jsonl");
        let body = format!("{}\n\n{}\n", sample_line(), sample_line());
        std::fs::write(&path, body).unwrap();
        let records = read_ledger(&path).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].run, 49);
    }

    #[test]
    fn a_malformed_line_surfaces_its_line_number() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ledger.jsonl");
        std::fs::write(&path, format!("{}\nnot-json\n", sample_line())).unwrap();
        let err = read_ledger(&path).unwrap_err();
        assert!(format!("{err:#}").contains("line 2"), "got: {err:#}");
    }

    #[test]
    fn an_unreadable_path_surfaces_a_reading_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let err = read_ledger(dir.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("reading ledger"),
            "got: {err:#}"
        );
    }
}
