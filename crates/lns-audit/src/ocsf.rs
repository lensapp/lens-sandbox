use anyhow::{Context, Result};
use serde_json::{Map, Value};

#[derive(Debug, Clone)]
pub struct Row {
    pub kind: String,
    pub detail: String,
    pub integration: Option<String>,
    pub run: String,
    pub ts: String,
}

pub fn read(event: &Map<String, Value>) -> Result<Row> {
    let um = unmapped(event)?;
    Ok(Row {
        kind: req(um, "lns_kind")?.to_string(),
        detail: event
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        integration: opt(um, "lns_integration"),
        run: text(um, "lns_run"),
        ts: text(um, "lns_ts"),
    })
}

pub fn microvm(event: &Map<String, Value>) -> String {
    unmapped(event)
        .map(|um| text(um, "lns_microvm"))
        .unwrap_or_default()
}

pub fn image(event: &Map<String, Value>) -> String {
    unmapped(event)
        .map(|um| text(um, "lns_image"))
        .unwrap_or_default()
}

fn unmapped(event: &Map<String, Value>) -> Result<&Map<String, Value>> {
    event
        .get("unmapped")
        .and_then(Value::as_object)
        .context("OCSF audit event has no unmapped object")
}

fn req<'a>(um: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    um.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("OCSF audit event missing string unmapped.{key}"))
}

fn text(um: &Map<String, Value>, key: &str) -> String {
    um.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn opt(um: &Map<String, Value>, key: &str) -> Option<String> {
    um.get(key).and_then(Value::as_str).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn octx(run: &str) -> lns_ocsf::Context<'_> {
        lns_ocsf::Context {
            time_unix_secs: 1_780_000_000,
            ts_rfc3339: "2026-06-29T14:00:00Z",
            run,
            microvm: "calm-finch",
        }
    }

    fn obj(value: &Value) -> Map<String, Value> {
        value.as_object().expect("event is an object").clone()
    }

    #[test]
    fn reads_the_kind_identity_and_the_events_message_as_the_detail() {
        let ev = lns_ocsf::connection(
            &octx("9e8d7c6b0000"),
            "some-oauth",
            "oauth",
            Some("@user"),
            &["repo".to_string()],
            None,
        );
        let row = read(&obj(&ev)).unwrap();
        assert_eq!(row.kind, "connection");
        assert_eq!(row.run, "9e8d7c6b0000");
        assert_eq!(row.ts, "2026-06-29T14:00:00Z");
        assert_eq!(row.integration.as_deref(), Some("some-oauth"));
        assert_eq!(
            row.detail, "connect some-oauth (oauth) @user [repo]",
            "detail is the event's own message, not re-derived by the reader"
        );
    }

    #[test]
    fn a_per_run_event_has_no_integration_and_surfaces_its_message() {
        let row = read(&obj(&lns_ocsf::volume_mount(&octx("r"), "data", "/data"))).unwrap();
        assert_eq!(row.kind, "volume");
        assert_eq!(row.detail, "data → /data");
        assert!(row.integration.is_none());
    }

    #[test]
    fn microvm_reads_the_device_name_for_scope_resolution() {
        assert_eq!(
            microvm(&obj(&lns_ocsf::volume_mount(&octx("r"), "d", "/d"))),
            "calm-finch"
        );
        assert_eq!(microvm(&Map::new()), "", "a non-OCSF object has no microVM");
    }

    #[test]
    fn image_reads_the_launch_event_image_and_is_empty_otherwise() {
        assert_eq!(
            image(&obj(&lns_ocsf::workload_launch(
                &octx("r"),
                "alpine:latest"
            ))),
            "alpine:latest"
        );
        assert_eq!(
            image(&obj(&lns_ocsf::volume_mount(&octx("r"), "d", "/d"))),
            "",
            "a non-launch event carries no image"
        );
        assert_eq!(image(&Map::new()), "", "a non-OCSF object has no image");
    }

    #[test]
    fn an_event_without_an_unmapped_object_is_rejected() {
        let err = read(&obj(&serde_json::json!({"class_uid": 3002}))).unwrap_err();
        assert!(format!("{err:#}").contains("no unmapped object"), "{err:#}");
    }

    #[test]
    fn an_event_missing_its_kind_is_rejected() {
        let err = read(&obj(&serde_json::json!({"unmapped": {"lns_run": "r"}}))).unwrap_err();
        assert!(format!("{err:#}").contains("lns_kind"), "{err:#}");
    }
}
