use serde_json::Value;

pub(super) fn is_empty_value(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(m) => m.is_empty(),
        _ => false,
    }
}

pub(super) fn is_structured(value: &Value) -> bool {
    match value {
        Value::Object(_) => true,
        Value::Array(items) => items
            .iter()
            .any(|e| matches!(e, Value::Object(_) | Value::Array(_))),
        _ => false,
    }
}

pub(super) fn render_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(render_value)
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

pub(super) fn field_label(key: &str) -> String {
    match key {
        "activity_name" => "Activity".to_string(),
        "category_name" => "Category".to_string(),
        "class_name" => "Class".to_string(),
        "type_name" => "Type".to_string(),
        "status_detail" => "Detail".to_string(),
        _ => humanize(key),
    }
}

fn humanize(key: &str) -> String {
    let mut chars = key.replace('_', " ").chars().collect::<Vec<_>>();
    if let Some(first) = chars.first_mut() {
        first.make_ascii_uppercase();
    }
    chars.into_iter().collect()
}

fn parse_ts(ts: &str) -> Option<i64> {
    crate::time_fmt::unix_from_rfc3339_opt(ts).map(|secs| secs as i64)
}

pub(super) fn friendly_time(now_secs: i64, ts: &str) -> String {
    let Some(event) = parse_ts(ts) else {
        return ts.to_string();
    };
    let delta = now_secs - event;
    if delta < 45 {
        "just now".to_string()
    } else if delta < 5_400 {
        format!("{}m ago", (delta / 60).max(1))
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3_600)
    } else if delta < 6 * 86_400 {
        format!("{}d ago", delta / 86_400)
    } else {
        ts.get(0..10).unwrap_or(ts).to_string()
    }
}

pub(super) fn relative_time(now_secs: i64, ts: &str) -> String {
    let Some(event) = parse_ts(ts) else {
        return String::new();
    };
    let delta = (now_secs - event).max(0);
    if delta < 45 {
        "just now".to_string()
    } else if delta < 5_400 {
        format!("{}m ago", (delta / 60).max(1))
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3_600)
    } else {
        format!("{}d ago", delta / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn is_empty_value_covers_each_json_shape() {
        assert!(is_empty_value(&Value::Null));
        assert!(is_empty_value(&json!("")));
        assert!(is_empty_value(&json!([])));
        assert!(is_empty_value(&json!({})));
        assert!(!is_empty_value(&json!("x")));
        assert!(!is_empty_value(&json!(["a"])));
        assert!(!is_empty_value(&json!({"k": 1})));
        assert!(!is_empty_value(&json!(0)));
    }

    #[test]
    fn is_structured_is_true_only_for_objects_and_nested_arrays() {
        assert!(is_structured(&json!({"k": 1})));
        assert!(is_structured(&json!([{"k": 1}])));
        assert!(is_structured(&json!([[1]])));
        assert!(!is_structured(&json!(["a", "b"])));
        assert!(!is_structured(&json!("scalar")));
        assert!(!is_structured(&json!(7)));
    }

    #[test]
    fn render_value_joins_arrays_and_stringifies_scalars() {
        assert_eq!(render_value(&json!("plain")), "plain");
        assert_eq!(render_value(&json!(["a", "b", "c"])), "a, b, c");
        assert_eq!(render_value(&json!(42)), "42");
        assert_eq!(render_value(&json!(true)), "true");
    }

    #[test]
    fn field_label_maps_the_special_keys_and_humanizes_the_rest() {
        assert_eq!(field_label("activity_name"), "Activity");
        assert_eq!(field_label("category_name"), "Category");
        assert_eq!(field_label("class_name"), "Class");
        assert_eq!(field_label("type_name"), "Type");
        assert_eq!(field_label("status_detail"), "Detail");
        assert_eq!(field_label("lns_origin"), "Lns origin");
        assert_eq!(field_label(""), "", "an empty key humanizes to empty");
    }

    const TS: &str = "2026-06-29T14:00:00Z";

    fn event() -> i64 {
        parse_ts(TS).expect("the fixture stamp parses")
    }

    #[test]
    fn friendly_time_returns_the_raw_stamp_when_unparseable() {
        assert_eq!(friendly_time(event(), "not-a-timestamp"), "not-a-timestamp");
    }

    #[test]
    fn friendly_time_walks_every_bucket_up_to_the_date_fallback() {
        assert_eq!(friendly_time(event() + 10, TS), "just now");
        assert_eq!(friendly_time(event() + 120, TS), "2m ago");
        assert_eq!(
            friendly_time(event() + 50, TS),
            "1m ago",
            "sub-minute rounds up"
        );
        assert_eq!(friendly_time(event() + 7_200, TS), "2h ago");
        assert_eq!(friendly_time(event() + 2 * 86_400, TS), "2d ago");
        assert_eq!(
            friendly_time(event() + 7 * 86_400, TS),
            "2026-06-29",
            "beyond six days it shows the date"
        );
    }

    #[test]
    fn relative_time_is_empty_when_unparseable_and_clamps_negative_deltas() {
        assert_eq!(relative_time(event(), "nope"), "");
        assert_eq!(
            relative_time(event() - 999, TS),
            "just now",
            "a future event clamps to just now, never a negative age"
        );
    }

    #[test]
    fn relative_time_walks_every_bucket_without_a_date_fallback() {
        assert_eq!(relative_time(event() + 10, TS), "just now");
        assert_eq!(relative_time(event() + 120, TS), "2m ago");
        assert_eq!(relative_time(event() + 50, TS), "1m ago");
        assert_eq!(relative_time(event() + 7_200, TS), "2h ago");
        assert_eq!(relative_time(event() + 9 * 86_400, TS), "9d ago");
    }
}
