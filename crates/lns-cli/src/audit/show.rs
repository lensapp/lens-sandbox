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
        let (event, detail) = describe(&obj);
        rows.push(vec![when(&obj), event, detail]);
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

fn describe(obj: &serde_json::Map<String, Value>) -> (String, String) {
    if obj.get("event").and_then(Value::as_str) == Some("run_env") {
        return ("env".to_string(), env_detail(obj));
    }
    if let Some(action) = obj.get("action").and_then(Value::as_str) {
        return ("egress".to_string(), egress_detail(action, obj));
    }
    match obj.get("type").and_then(Value::as_str) {
        Some("volume_attached") => ("volume".to_string(), volume_detail(obj)),
        Some("bind_attached") => ("bind".to_string(), bind_detail(obj)),
        _ => (label(obj), summary(obj)),
    }
}

fn egress_detail(action: &str, obj: &serde_json::Map<String, Value>) -> String {
    let mut detail = request_summary(action);
    if let Some(reason) = obj
        .get("metadata")
        .and_then(|m| m.get("reason"))
        .and_then(Value::as_str)
    {
        detail.push_str(&format!(" — {}", decision_phrase(reason)));
    }
    if let Some(outcome) = outcome_phrase(obj) {
        detail.push(' ');
        detail.push_str(&outcome);
    }
    detail
}

fn request_summary(action: &str) -> String {
    let (method, url) = action.split_once(' ').unwrap_or(("", action));
    let host = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(url);
    if method.is_empty() {
        host.to_string()
    } else {
        format!("{method} {host}")
    }
}

fn decision_phrase(reason: &str) -> &str {
    match reason {
        "user-allowed-once" => "allowed once",
        "user-allowed-persisted" => "allowed always",
        "user-denied-once" => "denied once",
        "user-denied-persisted" => "denied always",
        other => other,
    }
}

fn outcome_phrase(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let status = obj.get("status_code").and_then(Value::as_u64);
    let result = obj.get("result").and_then(Value::as_str);
    match (status, result) {
        (Some(s), Some(r)) => Some(format!("→ {s} {r}")),
        (Some(s), None) => Some(format!("→ {s}")),
        (None, Some(r)) => Some(format!("→ {r}")),
        (None, None) => None,
    }
}

fn env_detail(obj: &serde_json::Map<String, Value>) -> String {
    match obj.get("env").and_then(Value::as_object) {
        Some(env) => format!(
            "injected: {}",
            env.keys().cloned().collect::<Vec<_>>().join(", ")
        ),
        None => summary(obj),
    }
}

fn volume_detail(obj: &serde_json::Map<String, Value>) -> String {
    format!("{} → {}", str_field(obj, "name"), str_field(obj, "target"))
}

fn bind_detail(obj: &serde_json::Map<String, Value>) -> String {
    let mut detail = format!(
        "{} → {}",
        str_field(obj, "source"),
        str_field(obj, "target")
    );
    let exposed: Vec<&str> = obj
        .get("exposed_secrets")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !exposed.is_empty() {
        detail.push_str(&format!(" (exposed: {})", exposed.join(", ")));
    }
    detail
}

fn str_field<'a>(obj: &'a serde_json::Map<String, Value>, key: &str) -> &'a str {
    obj.get(key).and_then(Value::as_str).unwrap_or("?")
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
    fn host_events_render_in_plain_language_not_internal_labels() {
        let (_d, path) = write_log(&[
            r#"{"ts":"2026-06-29T14:01:02Z","event":"run_env","origin":"host","env":{"FOO":"bar"},"prev_hash":"00"}"#,
            r#"{"type":"volume_attached","name":"data","target":"/data","prev_hash":"ab"}"#,
        ]);
        let text = show_json_false("49", &path).unwrap();
        assert!(text.contains("WHEN"));
        assert!(text.contains("2026-06-29 14:01:02"));
        assert!(text.contains("env"));
        assert!(text.contains("injected: FOO"));
        assert!(text.contains("volume"));
        assert!(text.contains("data → /data"));
        assert!(!text.contains("prev_hash"));
        assert!(
            !text.contains("audit_event"),
            "the internal envelope label must not leak: {text}"
        );
    }

    #[test]
    fn an_event_without_a_timestamp_shows_a_dash() {
        let (_d, path) = write_log(&[r#"{"type":"bind_attached","source":"/a","prev_hash":"00"}"#]);
        let text = show_json_false("49", &path).unwrap();
        assert!(text.contains('-'));
        assert!(text.contains("bind"));
        assert!(text.contains("/a → ?"));
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

    #[test]
    fn an_egress_decision_reads_like_the_approval_prompt() {
        let (_d, path) = write_log(&[
            r#"{"ts":"2026-06-29T14:02:00Z","action":"GET http://www.google.com:80/","metadata":{"reason":"user-allowed-once"},"origin":"guest-proxy","prev_hash":"00"}"#,
        ]);
        let text = show_json_false("49", &path).unwrap();
        assert!(text.contains("egress"), "{text}");
        assert!(
            text.contains("GET www.google.com:80 — allowed once"),
            "{text}"
        );
        assert!(!text.contains("audit_event"), "{text}");
    }

    #[test]
    fn an_egress_outcome_shows_the_status_and_result() {
        let (_d, path) = write_log(&[
            r#"{"action":"GET http://www.google.com:80/","result":"success","status_code":200,"origin":"guest-proxy","prev_hash":"00"}"#,
        ]);
        let text = show_json_false("49", &path).unwrap();
        assert!(
            text.contains("GET www.google.com:80 → 200 success"),
            "{text}"
        );
    }

    #[test]
    fn an_egress_combines_decision_and_outcome() {
        let (_d, path) = write_log(&[
            r#"{"action":"GET http://denied.example:80/","metadata":{"reason":"user-denied-once"},"result":"failure","status_code":403,"prev_hash":"00"}"#,
        ]);
        let text = show_json_false("49", &path).unwrap();
        assert!(
            text.contains("GET denied.example:80 — denied once → 403 failure"),
            "{text}"
        );
    }

    #[test]
    fn an_egress_with_no_decision_or_outcome_is_just_the_request() {
        let (_d, path) =
            write_log(&[r#"{"action":"CONNECT api.some-oauth.example:443","prev_hash":"00"}"#]);
        let text = show_json_false("49", &path).unwrap();
        assert!(
            text.contains("CONNECT api.some-oauth.example:443"),
            "{text}"
        );
    }

    #[test]
    fn decision_phrase_uses_the_card_vocabulary_and_passes_unknowns_through() {
        assert_eq!(decision_phrase("user-allowed-once"), "allowed once");
        assert_eq!(decision_phrase("user-allowed-persisted"), "allowed always");
        assert_eq!(decision_phrase("user-denied-once"), "denied once");
        assert_eq!(decision_phrase("user-denied-persisted"), "denied always");
        assert_eq!(decision_phrase("some-new-reason"), "some-new-reason");
    }

    #[test]
    fn request_summary_strips_scheme_and_path_keeping_method_and_host() {
        assert_eq!(request_summary("GET http://h:80/x"), "GET h:80");
        assert_eq!(
            request_summary("https://api.example.test/v1"),
            "api.example.test"
        );
        assert_eq!(request_summary("CONNECT host:443"), "CONNECT host:443");
    }

    #[test]
    fn outcome_phrase_covers_status_and_result_combinations() {
        let mk = |json: &str| {
            outcome_phrase(&serde_json::from_str::<serde_json::Map<String, Value>>(json).unwrap())
        };
        assert_eq!(
            mk(r#"{"status_code":200,"result":"success"}"#).as_deref(),
            Some("→ 200 success")
        );
        assert_eq!(mk(r#"{"status_code":204}"#).as_deref(), Some("→ 204"));
        assert_eq!(mk(r#"{"result":"error"}"#).as_deref(), Some("→ error"));
        assert_eq!(mk("{}"), None);
    }

    #[test]
    fn a_bind_with_exposed_secrets_names_them() {
        let (_d, path) = write_log(&[
            r#"{"type":"bind_attached","source":"/proj","target":"/work","exposed_secrets":[".env"],"prev_hash":"00"}"#,
        ]);
        let text = show_json_false("49", &path).unwrap();
        assert!(text.contains("/proj → /work (exposed: .env)"), "{text}");
    }

    #[test]
    fn a_run_env_without_an_env_object_falls_back_to_its_fields() {
        let (_d, path) = write_log(&[r#"{"event":"run_env","foo":1,"prev_hash":"00"}"#]);
        let text = show_json_false("49", &path).unwrap();
        assert!(text.contains("env"), "{text}");
        assert!(text.contains("foo=1"), "{text}");
    }
}
