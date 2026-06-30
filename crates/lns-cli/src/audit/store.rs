use std::fs::File;
use std::io::{BufRead, BufReader, Lines};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lns_ipc::LedgerRecord;

/// Streams the ledger one record per line so callers fold or filter without holding the whole file in memory; a missing ledger streams as empty.
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
    type Item = Result<LedgerRecord>;

    fn next(&mut self) -> Option<Self::Item> {
        let lines = self.lines.as_mut()?;
        loop {
            let line = match lines.next()? {
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
                serde_json::from_str(&line)
                    .with_context(|| format!("parsing ledger line {line_no} of {path}")),
            );
        }
    }
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

    fn collect(path: &Path) -> Result<Vec<LedgerRecord>> {
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
        assert_eq!(records[0].run, 49);
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
