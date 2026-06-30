use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::ShowArgs;
use super::friendly_when;
use super::table::render_table;

pub fn run(args: &ShowArgs, out: &mut dyn Write) -> Result<i32> {
    let path = lns_ipc::audit_log_for_run(&args.run_id)?;
    run_with_path(args, &path, out)
}

fn run_with_path(args: &ShowArgs, path: &Path, out: &mut dyn Write) -> Result<i32> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "no audit log for run {} (looked in {})",
                args.run_id,
                path.display()
            );
        }
        Err(e) => {
            return Err(e).with_context(|| format!("reading audit log {}", path.display()));
        }
    };
    super::warn_if_compromised(path, &path.with_file_name("audit.anchor"));
    if args.json {
        write!(out, "{text}")?;
        return Ok(0);
    }
    render_text(&text, &args.run_id, path, out)?;
    Ok(0)
}

fn render_text(text: &str, run_id: &str, path: &Path, out: &mut dyn Write) -> Result<()> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .with_context(|| format!("parsing audit line {} of {}", idx + 1, path.display()))?;
        let Value::Object(obj) = value else {
            bail!(
                "audit line {} of {} is not a JSON object",
                idx + 1,
                path.display()
            );
        };
        rows.push(vec![when(&obj), label(&obj), summary(&obj)]);
    }
    if rows.is_empty() {
        writeln!(out, "No audit events for run {run_id}.")?;
        return Ok(());
    }
    render_table(out, &["WHEN", "EVENT", "DETAIL"], &rows)?;
    Ok(())
}

fn when(obj: &serde_json::Map<String, Value>) -> String {
    match obj.get("ts").and_then(Value::as_str) {
        Some(ts) => friendly_when(ts),
        None => "-".to_string(),
    }
}

fn label(obj: &serde_json::Map<String, Value>) -> String {
    obj.get("event")
        .or_else(|| obj.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("event")
        .to_string()
}

fn summary(obj: &serde_json::Map<String, Value>) -> String {
    obj.iter()
        .filter(|(k, _)| !matches!(k.as_str(), "prev_hash" | "type" | "event" | "ts"))
        .map(|(k, v)| format!("{k}={}", scalar(v)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show_json_false(run_id: &str, path: &Path) -> Result<String> {
        let mut buf = Vec::new();
        run_with_path(
            &ShowArgs {
                run_id: run_id.into(),
                json: false,
            },
            path,
            &mut buf,
        )?;
        Ok(String::from_utf8(buf).unwrap())
    }

    fn write_log(lines: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("audit.jsonl");
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
        (dir, path)
    }

    #[test]
    fn a_missing_run_log_bails_with_a_helpful_message() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("audit.jsonl");
        let err = show_json_false("424242", &path).unwrap_err();
        assert!(format!("{err:#}").contains("no audit log for run 424242"));
    }

    #[test]
    fn each_event_renders_with_its_timestamp_label_and_fields() {
        let (_d, path) = write_log(&[
            r#"{"ts":"2026-06-29T14:01:02Z","event":"run_env","env":{"FOO":"bar"},"prev_hash":"00"}"#,
            r#"{"type":"volume_attached","name":"data","target":"/data","prev_hash":"ab"}"#,
        ]);
        let text = show_json_false("49", &path).unwrap();
        assert!(text.contains("WHEN"));
        assert!(text.contains("2026-06-29 14:01:02"));
        assert!(text.contains("run_env"));
        assert!(text.contains("env={\"FOO\":\"bar\"}"));
        assert!(text.contains("volume_attached"));
        assert!(text.contains("name=data"));
        assert!(text.contains("target=/data"));
        assert!(!text.contains("prev_hash"));
    }

    #[test]
    fn an_event_without_a_timestamp_shows_a_dash() {
        let (_d, path) = write_log(&[r#"{"type":"bind_attached","source":"/a","prev_hash":"00"}"#]);
        let text = show_json_false("49", &path).unwrap();
        assert!(text.contains('-'));
        assert!(text.contains("bind_attached"));
    }

    #[test]
    fn an_event_lacking_event_and_type_falls_back_to_a_generic_label() {
        let (_d, path) = write_log(&[r#"{"foo":1,"prev_hash":"00"}"#]);
        let text = show_json_false("49", &path).unwrap();
        assert!(text.contains("event"));
        assert!(text.contains("foo=1"));
    }

    #[test]
    fn an_empty_log_reports_no_events() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("audit.jsonl");
        std::fs::write(&path, "\n\n").unwrap();
        let text = show_json_false("49", &path).unwrap();
        assert_eq!(text.trim(), "No audit events for run 49.");
    }

    #[test]
    fn a_non_object_line_is_reported_as_corruption() {
        let (_d, path) = write_log(&["42"]);
        let err = show_json_false("49", &path).unwrap_err();
        assert!(format!("{err:#}").contains("not a JSON object"));
    }

    #[test]
    fn a_malformed_line_surfaces_its_line_number() {
        let (_d, path) = write_log(&["not json"]);
        let err = show_json_false("49", &path).unwrap_err();
        assert!(format!("{err:#}").contains("audit line 1"));
    }

    #[test]
    fn json_mode_echoes_the_raw_log() {
        let raw = r#"{"type":"volume_attached","prev_hash":"00"}"#;
        let (_d, path) = write_log(&[raw]);
        let mut buf = Vec::new();
        run_with_path(
            &ShowArgs {
                run_id: "49".into(),
                json: true,
            },
            &path,
            &mut buf,
        )
        .unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains(raw));
    }

    #[test]
    fn an_unreadable_path_surfaces_a_reading_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let err = show_json_false("49", dir.path()).unwrap_err();
        assert!(format!("{err:#}").contains("reading audit log"));
    }
}
