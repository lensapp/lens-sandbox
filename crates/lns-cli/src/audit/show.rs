use serde_json::Value;

use super::friendly_when;

pub(super) fn when(obj: &serde_json::Map<String, Value>) -> String {
    match obj.get("ts").and_then(Value::as_str) {
        Some(ts) => friendly_when(ts),
        None => "-".to_string(),
    }
}

pub(super) fn describe(obj: &serde_json::Map<String, Value>) -> (String, String) {
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
        .filter(|(k, _)| !matches!(k.as_str(), "prev_hash" | "type" | "event" | "ts" | "run"))
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

    fn obj(json: &str) -> serde_json::Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn host_events_describe_in_plain_language_not_internal_labels() {
        let (kind, detail) = describe(&obj(
            r#"{"ts":"2026-06-29T14:01:02Z","event":"run_env","origin":"host","env":{"FOO":"bar"},"prev_hash":"00"}"#,
        ));
        assert_eq!(kind, "env");
        assert_eq!(detail, "injected: FOO");
        let (kind, detail) = describe(&obj(
            r#"{"type":"volume_attached","name":"data","target":"/data","prev_hash":"ab"}"#,
        ));
        assert_eq!(kind, "volume");
        assert_eq!(detail, "data → /data");
    }

    #[test]
    fn when_renders_the_timestamp_or_a_dash() {
        assert_eq!(
            when(&obj(r#"{"ts":"2026-06-29T14:01:02Z"}"#)),
            "2026-06-29 14:01:02"
        );
        assert_eq!(when(&obj(r#"{"type":"bind_attached"}"#)), "-");
    }

    #[test]
    fn a_bind_without_a_target_shows_a_question_mark() {
        let (kind, detail) = describe(&obj(
            r#"{"type":"bind_attached","source":"/a","prev_hash":"00"}"#,
        ));
        assert_eq!(kind, "bind");
        assert_eq!(detail, "/a → ?");
    }

    #[test]
    fn an_event_lacking_event_and_type_falls_back_to_a_generic_label() {
        let (kind, detail) = describe(&obj(r#"{"note":"hi","count":1,"prev_hash":"00"}"#));
        assert_eq!(kind, "event");
        assert!(detail.contains("note=hi"));
        assert!(detail.contains("count=1"));
    }

    #[test]
    fn an_egress_decision_reads_like_the_approval_prompt() {
        let (kind, detail) = describe(&obj(
            r#"{"ts":"2026-06-29T14:02:00Z","action":"GET http://www.google.com:80/","metadata":{"reason":"user-allowed-once"},"origin":"guest-proxy","prev_hash":"00"}"#,
        ));
        assert_eq!(kind, "egress");
        assert_eq!(detail, "GET www.google.com:80 — allowed once");
    }

    #[test]
    fn an_egress_outcome_shows_the_status_and_result() {
        let (_, detail) = describe(&obj(
            r#"{"action":"GET http://www.google.com:80/","result":"success","status_code":200,"origin":"guest-proxy","prev_hash":"00"}"#,
        ));
        assert_eq!(detail, "GET www.google.com:80 → 200 success");
    }

    #[test]
    fn an_egress_combines_decision_and_outcome() {
        let (_, detail) = describe(&obj(
            r#"{"action":"GET http://denied.example:80/","metadata":{"reason":"user-denied-once"},"result":"failure","status_code":403,"prev_hash":"00"}"#,
        ));
        assert_eq!(detail, "GET denied.example:80 — denied once → 403 failure");
    }

    #[test]
    fn an_egress_with_no_decision_or_outcome_is_just_the_request() {
        let (_, detail) = describe(&obj(
            r#"{"action":"CONNECT api.some-oauth.example:443","prev_hash":"00"}"#,
        ));
        assert_eq!(detail, "CONNECT api.some-oauth.example:443");
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
        let mk = |json: &str| outcome_phrase(&obj(json));
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
        let (_, detail) = describe(&obj(
            r#"{"type":"bind_attached","source":"/proj","target":"/work","exposed_secrets":[".env"],"prev_hash":"00"}"#,
        ));
        assert_eq!(detail, "/proj → /work (exposed: .env)");
    }

    #[test]
    fn a_run_env_without_an_env_object_falls_back_to_its_fields() {
        let (kind, detail) = describe(&obj(r#"{"event":"run_env","foo":1,"prev_hash":"00"}"#));
        assert_eq!(kind, "env");
        assert!(detail.contains("foo=1"), "{detail}");
    }

    #[test]
    fn the_injected_run_field_is_not_echoed_in_the_generic_summary() {
        let (_, detail) = describe(&obj(r#"{"note":"hi","run":"abc","prev_hash":"00"}"#));
        assert!(detail.contains("note=hi"));
        assert!(
            !detail.contains("run=abc"),
            "the synthetic run field must not leak: {detail}"
        );
    }
}
