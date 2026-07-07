use serde_json::{Map, Value, json};

const VERSION: &str = "1.7.0";
const PRODUCT_NAME: &str = "lens-sandbox";
const PRODUCT_VENDOR: &str = "Mirantis";
const CLOUD_PROVIDER: &str = "lens-sandbox";

pub mod class {
    pub const AUTHENTICATION: u32 = 3002;
    pub const NETWORK_ACTIVITY: u32 = 4001;
    pub const HTTP_ACTIVITY: u32 = 4002;
    pub const PROCESS_ACTIVITY: u32 = 1007;
    pub const FILE_ACTIVITY: u32 = 1001;
    pub const DETECTION_FINDING: u32 = 2004;
}

pub mod category {
    pub const SYSTEM: u32 = 1;
    pub const FINDINGS: u32 = 2;
    pub const IAM: u32 = 3;
    pub const NETWORK: u32 = 4;
}

pub mod activity {
    pub const LOGON: u16 = 1;
    pub const FINDING_CREATE: u16 = 1;
    pub const NETWORK_OPEN: u16 = 1;
    pub const NETWORK_CLOSE: u16 = 2;
    pub const PROCESS_LAUNCH: u16 = 1;
    pub const FILE_MOUNT: u16 = 12;
}

pub mod severity {
    pub const INFORMATIONAL: u8 = 1;
    pub const MEDIUM: u8 = 3;
}

pub mod status {
    pub const SUCCESS: u8 = 1;
    pub const FAILURE: u8 = 2;
}

pub mod disposition {
    pub const ALLOWED: u8 = 1;
    pub const BLOCKED: u8 = 2;
}

pub mod device_type {
    pub const VIRTUAL: u8 = 6;
}

pub mod file_type {
    pub const FOLDER: u8 = 2;
}

pub fn http_method_activity(method: &str) -> u16 {
    match method.to_ascii_uppercase().as_str() {
        "CONNECT" => 1,
        "DELETE" => 2,
        "GET" => 3,
        "HEAD" => 4,
        "OPTIONS" => 5,
        "POST" => 6,
        "PUT" => 7,
        "TRACE" => 8,
        "PATCH" => 9,
        _ => 99,
    }
}

fn class_name(class_uid: u32) -> &'static str {
    match class_uid {
        class::FILE_ACTIVITY => "File System Activity",
        class::PROCESS_ACTIVITY => "Process Activity",
        class::DETECTION_FINDING => "Detection Finding",
        class::AUTHENTICATION => "Authentication",
        class::NETWORK_ACTIVITY => "Network Activity",
        class::HTTP_ACTIVITY => "HTTP Activity",
        _ => "Unknown",
    }
}

fn category_name(category_uid: u32) -> &'static str {
    match category_uid {
        category::SYSTEM => "System Activity",
        category::FINDINGS => "Findings",
        category::IAM => "Identity & Access Management",
        category::NETWORK => "Network Activity",
        _ => "Unknown",
    }
}

fn activity_name(class_uid: u32, activity_id: u16) -> &'static str {
    match (class_uid, activity_id) {
        (class::AUTHENTICATION, activity::LOGON) => "Logon",
        (class::DETECTION_FINDING, activity::FINDING_CREATE) => "Create",
        (class::PROCESS_ACTIVITY, activity::PROCESS_LAUNCH) => "Launch",
        (class::FILE_ACTIVITY, activity::FILE_MOUNT) => "Mount",
        (class::NETWORK_ACTIVITY, activity::NETWORK_OPEN) => "Open",
        (class::NETWORK_ACTIVITY, activity::NETWORK_CLOSE) => "Close",
        (class::HTTP_ACTIVITY, id) => http_activity_name(id),
        _ => "Unknown",
    }
}

fn http_activity_name(activity_id: u16) -> &'static str {
    match activity_id {
        1 => "Connect",
        2 => "Delete",
        3 => "Get",
        4 => "Head",
        5 => "Options",
        6 => "Post",
        7 => "Put",
        8 => "Trace",
        9 => "Patch",
        _ => "Other",
    }
}

fn severity_name(severity_id: u8) -> &'static str {
    match severity_id {
        severity::INFORMATIONAL => "Informational",
        severity::MEDIUM => "Medium",
        _ => "Unknown",
    }
}

fn status_name(status_id: u8) -> &'static str {
    match status_id {
        status::SUCCESS => "Success",
        status::FAILURE => "Failure",
        _ => "Unknown",
    }
}

fn disposition_name(disposition_id: u8) -> &'static str {
    match disposition_id {
        disposition::ALLOWED => "Allowed",
        disposition::BLOCKED => "Blocked",
        _ => "Unknown",
    }
}

pub struct Context<'a> {
    pub time_unix_secs: u64,
    pub ts_rfc3339: &'a str,
    pub run: &'a str,
    pub microvm: &'a str,
}

pub struct Event {
    top: Map<String, Value>,
    unmapped: Map<String, Value>,
}

impl Event {
    pub fn new(
        kind: &str,
        class_uid: u32,
        category_uid: u32,
        activity_id: u16,
        severity_id: u8,
        ctx: &Context,
    ) -> Self {
        let type_uid = u64::from(class_uid) * 100 + u64::from(activity_id);
        let time_ms = i64::try_from(ctx.time_unix_secs)
            .unwrap_or(i64::MAX)
            .saturating_mul(1000);
        let mut top = Map::new();
        top.insert("class_uid".into(), class_uid.into());
        top.insert("class_name".into(), class_name(class_uid).into());
        top.insert("category_uid".into(), category_uid.into());
        top.insert("category_name".into(), category_name(category_uid).into());
        top.insert("type_uid".into(), type_uid.into());
        top.insert(
            "type_name".into(),
            format!(
                "{}: {}",
                class_name(class_uid),
                activity_name(class_uid, activity_id)
            )
            .into(),
        );
        top.insert("activity_id".into(), activity_id.into());
        top.insert(
            "activity_name".into(),
            activity_name(class_uid, activity_id).into(),
        );
        top.insert("severity_id".into(), severity_id.into());
        top.insert("severity".into(), severity_name(severity_id).into());
        top.insert("time".into(), time_ms.into());
        top.insert(
            "metadata".into(),
            json!({"version": VERSION, "product": {"name": PRODUCT_NAME, "vendor_name": PRODUCT_VENDOR}}),
        );
        top.insert("cloud".into(), json!({"provider": CLOUD_PROVIDER}));
        top.insert("osint".into(), Value::Array(Vec::new()));
        let mut unmapped = Map::new();
        unmapped.insert("lns_kind".into(), kind.into());
        unmapped.insert("lns_run".into(), ctx.run.into());
        unmapped.insert("lns_microvm".into(), ctx.microvm.into());
        unmapped.insert("lns_ts".into(), ctx.ts_rfc3339.into());
        Self { top, unmapped }
    }

    pub fn set(mut self, key: &str, value: Value) -> Self {
        self.top.insert(key.into(), value);
        self
    }

    pub fn set_status(self, status_id: u8) -> Self {
        self.set("status_id", status_id.into())
            .set("status", status_name(status_id).into())
    }

    pub fn set_disposition(self, disposition_id: u8) -> Self {
        self.set("disposition_id", disposition_id.into())
            .set("disposition", disposition_name(disposition_id).into())
    }

    pub fn note(mut self, key: &str, value: Value) -> Self {
        self.unmapped.insert(key.into(), value);
        self
    }

    pub fn build(mut self) -> Value {
        self.top
            .insert("unmapped".into(), Value::Object(self.unmapped));
        Value::Object(self.top)
    }
}

#[cfg(test)]
pub(crate) fn assert_schema_valid(ev: &Value) {
    let o = ev.as_object().expect("event is a JSON object");
    for key in [
        "activity_id",
        "activity_name",
        "category_uid",
        "category_name",
        "class_uid",
        "class_name",
        "cloud",
        "metadata",
        "osint",
        "severity_id",
        "severity",
        "time",
        "type_uid",
        "type_name",
        "unmapped",
    ] {
        assert!(o.contains_key(key), "missing required base field {key}");
    }
    let class_uid = o["class_uid"].as_u64().expect("class_uid is a number");
    let activity_id = o["activity_id"].as_u64().expect("activity_id is a number");
    assert_eq!(
        o["type_uid"].as_u64().expect("type_uid is a number"),
        class_uid * 100 + activity_id,
        "type_uid must be class_uid*100 + activity_id"
    );
    assert_eq!(
        o["type_name"].as_str().expect("type_name is a string"),
        format!(
            "{}: {}",
            o["class_name"].as_str().unwrap(),
            o["activity_name"].as_str().unwrap()
        ),
        "type_name must read as 'class: activity'"
    );
    assert_eq!(o["metadata"]["version"], "1.7.0");
    assert!(o["metadata"]["product"]["name"].is_string());
    assert!(o["cloud"]["provider"].is_string());
    assert!(o["osint"].is_array());
    assert!(o["time"].as_i64().expect("time is an integer") >= 0);
    assert!(o["unmapped"]["lns_kind"].is_string());
    assert!(o["unmapped"]["lns_run"].is_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context<'static> {
        Context {
            time_unix_secs: 1_780_000_000,
            ts_rfc3339: "2026-06-29T14:00:00Z",
            run: "9e8d7c6b0000",
            microvm: "calm-finch",
        }
    }

    #[test]
    fn class_and_category_captions_map_every_uid() {
        assert_eq!(class_name(class::FILE_ACTIVITY), "File System Activity");
        assert_eq!(class_name(class::PROCESS_ACTIVITY), "Process Activity");
        assert_eq!(class_name(class::DETECTION_FINDING), "Detection Finding");
        assert_eq!(class_name(class::AUTHENTICATION), "Authentication");
        assert_eq!(class_name(class::NETWORK_ACTIVITY), "Network Activity");
        assert_eq!(class_name(class::HTTP_ACTIVITY), "HTTP Activity");
        assert_eq!(class_name(9999), "Unknown");

        assert_eq!(category_name(category::SYSTEM), "System Activity");
        assert_eq!(category_name(category::FINDINGS), "Findings");
        assert_eq!(category_name(category::IAM), "Identity & Access Management");
        assert_eq!(category_name(category::NETWORK), "Network Activity");
        assert_eq!(category_name(9), "Unknown");
    }

    #[test]
    fn activity_captions_map_each_class_and_the_http_methods() {
        assert_eq!(
            activity_name(class::AUTHENTICATION, activity::LOGON),
            "Logon"
        );
        assert_eq!(
            activity_name(class::DETECTION_FINDING, activity::FINDING_CREATE),
            "Create"
        );
        assert_eq!(
            activity_name(class::PROCESS_ACTIVITY, activity::PROCESS_LAUNCH),
            "Launch"
        );
        assert_eq!(
            activity_name(class::FILE_ACTIVITY, activity::FILE_MOUNT),
            "Mount"
        );
        assert_eq!(
            activity_name(class::NETWORK_ACTIVITY, activity::NETWORK_OPEN),
            "Open"
        );
        assert_eq!(
            activity_name(class::NETWORK_ACTIVITY, activity::NETWORK_CLOSE),
            "Close"
        );
        assert_eq!(activity_name(class::HTTP_ACTIVITY, 3), "Get");
        assert_eq!(activity_name(9999, 0), "Unknown");

        for (id, name) in [
            (1, "Connect"),
            (2, "Delete"),
            (3, "Get"),
            (4, "Head"),
            (5, "Options"),
            (6, "Post"),
            (7, "Put"),
            (8, "Trace"),
            (9, "Patch"),
            (99, "Other"),
        ] {
            assert_eq!(http_activity_name(id), name);
        }
    }

    #[test]
    fn severity_status_and_disposition_captions_map_every_id() {
        assert_eq!(severity_name(severity::INFORMATIONAL), "Informational");
        assert_eq!(severity_name(severity::MEDIUM), "Medium");
        assert_eq!(severity_name(9), "Unknown");
        assert_eq!(status_name(status::SUCCESS), "Success");
        assert_eq!(status_name(status::FAILURE), "Failure");
        assert_eq!(status_name(9), "Unknown");
        assert_eq!(disposition_name(disposition::ALLOWED), "Allowed");
        assert_eq!(disposition_name(disposition::BLOCKED), "Blocked");
        assert_eq!(disposition_name(9), "Unknown");
    }

    #[test]
    fn the_base_event_carries_the_required_fields_and_computes_type_uid() {
        let ev = Event::new(
            "connection",
            class::AUTHENTICATION,
            category::IAM,
            activity::LOGON,
            severity::INFORMATIONAL,
            &ctx(),
        )
        .build();
        assert_schema_valid(&ev);
        assert_eq!(ev["type_uid"], 300201);
        assert_eq!(ev["class_name"], "Authentication");
        assert_eq!(ev["category_name"], "Identity & Access Management");
        assert_eq!(ev["activity_name"], "Logon");
        assert_eq!(ev["type_name"], "Authentication: Logon");
        assert_eq!(ev["severity"], "Informational");
        assert_eq!(ev["time"], 1_780_000_000_000i64);
        assert_eq!(ev["unmapped"]["lns_microvm"], "calm-finch");
        assert_eq!(ev["unmapped"]["lns_ts"], "2026-06-29T14:00:00Z");
    }

    #[test]
    fn set_and_note_place_fields_at_top_level_and_under_unmapped() {
        let ev = Event::new(
            "egress",
            class::HTTP_ACTIVITY,
            category::NETWORK,
            activity::LOGON,
            severity::INFORMATIONAL,
            &ctx(),
        )
        .set("status_id", Value::from(status::SUCCESS))
        .note("lns_reason", "user-allowed-once".into())
        .build();
        assert_eq!(ev["status_id"], 1);
        assert_eq!(ev["unmapped"]["lns_reason"], "user-allowed-once");
    }

    #[test]
    fn http_method_maps_to_the_matching_activity_id() {
        assert_eq!(http_method_activity("GET"), 3);
        assert_eq!(http_method_activity("post"), 6);
        assert_eq!(http_method_activity("PUT"), 7);
        assert_eq!(http_method_activity("DELETE"), 2);
        assert_eq!(http_method_activity("HEAD"), 4);
        assert_eq!(http_method_activity("OPTIONS"), 5);
        assert_eq!(http_method_activity("CONNECT"), 1);
        assert_eq!(http_method_activity("TRACE"), 8);
        assert_eq!(http_method_activity("PATCH"), 9);
        assert_eq!(http_method_activity("BREW"), 99);
    }

    #[test]
    fn an_out_of_range_timestamp_saturates_instead_of_panicking() {
        let ev = Event::new(
            "connection",
            class::AUTHENTICATION,
            category::IAM,
            activity::LOGON,
            severity::INFORMATIONAL,
            &Context {
                time_unix_secs: u64::MAX,
                ts_rfc3339: "",
                run: "",
                microvm: "",
            },
        )
        .build();
        assert_eq!(ev["time"], i64::MAX);
    }
}
