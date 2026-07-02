use std::fs::File;
use std::io::{BufRead, BufReader, Lines};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

/// Streams the ledger one OCSF event per line (`prev_hash` stripped) so callers fold or filter without holding the whole file in memory; a missing ledger streams as empty.
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
    type Item = Result<Map<String, Value>>;

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

fn parse_ledger_line(line: &str) -> Result<Map<String, Value>> {
    let Value::Object(mut obj) = serde_json::from_str::<Value>(line)? else {
        bail!("ledger line is not a JSON object");
    };
    obj.remove("prev_hash");
    Ok(obj)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_line() -> String {
        lns_ocsf::credential_use(
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
        )
        .to_string()
    }

    fn collect(path: &Path) -> Result<Vec<Map<String, Value>>> {
        stream_ledger(path)?.collect()
    }

    #[test]
    fn a_missing_ledger_streams_as_no_events_not_an_error() {
        assert!(
            collect(Path::new("/nope/never/ledger.jsonl"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn blank_lines_are_skipped_and_events_parse() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ledger.jsonl");
        std::fs::write(&path, format!("{}\n\n{}\n", sample_line(), sample_line())).unwrap();
        let events = collect(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0]["unmapped"]["lns_run"],
            "5e6f7a8b0000000000000000000000bb"
        );
    }

    #[test]
    fn a_chained_line_is_yielded_as_its_ocsf_event_with_the_chain_link_stripped() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ledger.jsonl");
        let mut chain = lns_ipc::AuditChain::new();
        std::fs::write(&path, chain.augment(&sample_line()).unwrap()).unwrap();

        let events = collect(&path).unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event["class_uid"], 3002);
        assert_eq!(event["unmapped"]["lns_kind"], "credential");
        assert_eq!(event.get("prev_hash"), None, "chain link leaked: {event:?}");
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
