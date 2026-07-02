use anyhow::{Context, Result, bail};
use lns_ipc::{LedgerEvent, LedgerRecord};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

pub(crate) fn is_ocsf(obj: &Map<String, Value>) -> bool {
    obj.contains_key("class_uid")
}

pub(crate) fn ledger_record(obj: &Map<String, Value>) -> Result<LedgerRecord> {
    let um = unmapped(obj)?;
    let event = match req_str(um, "lns_kind")?.as_str() {
        "connection" => connection_event(um)?,
        "credential" => credential_event(um)?,
        "approval" => approval_event(um)?,
        other => bail!("OCSF event unmapped.lns_kind {other:?} is not a ledger kind"),
    };
    Ok(LedgerRecord {
        ts: req_str(um, "lns_ts")?,
        run: req_str(um, "lns_run")?,
        microvm: req_str(um, "lns_microvm")?,
        event,
    })
}

pub(crate) fn runlog_obj(obj: &Map<String, Value>) -> Result<Map<String, Value>> {
    let um = unmapped(obj)?;
    let mut legacy = match req_str(um, "lns_kind")?.as_str() {
        "egress" => egress_legacy(obj, um)?,
        "env" => env_legacy(um)?,
        "volume" => volume_legacy(um)?,
        "bind" => bind_legacy(um)?,
        other => bail!("OCSF event unmapped.lns_kind {other:?} is not a per-run kind"),
    };
    legacy.insert("ts".to_string(), req_str(um, "lns_ts")?.into());
    Ok(legacy)
}

fn connection_event(um: &Map<String, Value>) -> Result<LedgerEvent> {
    Ok(LedgerEvent::Connection {
        integration: req_str(um, "lns_integration")?,
        auth: parse(um, "lns_auth")?,
        account: opt_str(um, "lns_account"),
        scopes: str_vec(um, "lns_scopes"),
        expires: opt_str(um, "lns_expires"),
    })
}

fn credential_event(um: &Map<String, Value>) -> Result<LedgerEvent> {
    Ok(LedgerEvent::CredentialUse {
        integration: req_str(um, "lns_integration")?,
        auth: parse(um, "lns_auth")?,
        fp: opt_str(um, "lns_fp"),
        dest: str_vec(um, "lns_dest"),
    })
}

fn approval_event(um: &Map<String, Value>) -> Result<LedgerEvent> {
    Ok(LedgerEvent::Approval {
        kind: parse(um, "lns_approval_kind")?,
        target: req_str(um, "lns_target")?,
        decision: parse(um, "lns_decision")?,
        reason: opt_str(um, "lns_reason"),
        integration: opt_str(um, "lns_integration"),
    })
}

fn egress_legacy(obj: &Map<String, Value>, um: &Map<String, Value>) -> Result<Map<String, Value>> {
    let method = obj
        .get("http_request")
        .and_then(|r| r.get("http_method"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let url = obj
        .get("http_request")
        .and_then(|r| r.get("url"))
        .and_then(|u| u.get("text"))
        .and_then(Value::as_str)
        .context("OCSF egress event has no http_request.url.text")?;
    let action = if method.is_empty() {
        url.to_string()
    } else {
        format!("{method} {url}")
    };
    let mut legacy = Map::new();
    legacy.insert("action".to_string(), action.into());
    if let Some(code) = obj
        .get("http_response")
        .and_then(|r| r.get("code"))
        .and_then(Value::as_u64)
    {
        legacy.insert("status_code".to_string(), code.into());
    }
    if let Some(result) = opt_str(um, "lns_result") {
        legacy.insert("result".to_string(), result.into());
    }
    if let Some(reason) = opt_str(um, "lns_reason") {
        legacy.insert("metadata".to_string(), json!({ "reason": reason }));
    }
    if let Some(origin) = opt_str(um, "lns_origin") {
        legacy.insert("origin".to_string(), origin.into());
    }
    Ok(legacy)
}

fn env_legacy(um: &Map<String, Value>) -> Result<Map<String, Value>> {
    let env = um
        .get("lns_env")
        .cloned()
        .context("OCSF env event has no unmapped.lns_env")?;
    let mut legacy = Map::new();
    legacy.insert("event".to_string(), "run_env".into());
    legacy.insert("env".to_string(), env);
    if let Some(origin) = opt_str(um, "lns_origin") {
        legacy.insert("origin".to_string(), origin.into());
    }
    Ok(legacy)
}

fn volume_legacy(um: &Map<String, Value>) -> Result<Map<String, Value>> {
    let mut legacy = Map::new();
    legacy.insert("type".to_string(), "volume_attached".into());
    legacy.insert("name".to_string(), req_str(um, "lns_name")?.into());
    legacy.insert("target".to_string(), req_str(um, "lns_target")?.into());
    Ok(legacy)
}

fn bind_legacy(um: &Map<String, Value>) -> Result<Map<String, Value>> {
    let mut legacy = Map::new();
    legacy.insert("type".to_string(), "bind_attached".into());
    legacy.insert("source".to_string(), req_str(um, "lns_source")?.into());
    legacy.insert("target".to_string(), req_str(um, "lns_target")?.into());
    legacy.insert(
        "exposed_secrets".to_string(),
        json!(str_vec(um, "lns_exposed_secrets")),
    );
    legacy.insert(
        "dropped_secrets".to_string(),
        json!(str_vec(um, "lns_dropped_secrets")),
    );
    Ok(legacy)
}

fn unmapped(obj: &Map<String, Value>) -> Result<&Map<String, Value>> {
    obj.get("unmapped")
        .and_then(Value::as_object)
        .context("OCSF event has no unmapped object")
}

fn req_str(um: &Map<String, Value>, key: &str) -> Result<String> {
    Ok(um
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("OCSF event missing string unmapped.{key}"))?
        .to_string())
}

fn opt_str(um: &Map<String, Value>, key: &str) -> Option<String> {
    um.get(key).and_then(Value::as_str).map(str::to_string)
}

fn str_vec(um: &Map<String, Value>, key: &str) -> Vec<String> {
    um.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse<T: DeserializeOwned>(um: &Map<String, Value>, key: &str) -> Result<T> {
    let value = um
        .get(key)
        .with_context(|| format!("OCSF event missing unmapped.{key}"))?;
    serde_json::from_value(value.clone())
        .with_context(|| format!("OCSF event unmapped.{key} is not a recognised value"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{events, show};
    use lns_ipc::{ApprovalKind, AuthKind, Decision, LedgerEvent};

    fn octx() -> lns_ocsf::Context<'static> {
        lns_ocsf::Context {
            time_unix_secs: 1_780_000_000,
            ts_rfc3339: "2026-06-29T14:00:00Z",
            run: "9e8d7c6b0000",
            microvm: "calm-finch",
        }
    }

    fn obj(value: &Value) -> Map<String, Value> {
        value.as_object().expect("event is an object").clone()
    }

    fn legacy_obj(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).expect("valid legacy json")
    }

    #[test]
    fn is_ocsf_keys_off_the_class_uid_marker() {
        assert!(is_ocsf(&obj(&lns_ocsf::volume_mount(
            &octx(),
            "data",
            "/data"
        ))));
        assert!(!is_ocsf(&legacy_obj(
            r#"{"type":"volume_attached","name":"data","target":"/data"}"#
        )));
    }

    #[test]
    fn a_connection_reconstructs_the_exact_ledger_event() {
        let scopes = vec!["repo".to_string(), "read:org".to_string()];
        let ev = lns_ocsf::connection(
            &octx(),
            "some-oauth",
            "oauth",
            Some("@user"),
            &scopes,
            Some("2026-07-29T00:00:00Z"),
        );
        let record = ledger_record(&obj(&ev)).unwrap();
        assert_eq!(record.run, "9e8d7c6b0000");
        assert_eq!(record.microvm, "calm-finch");
        assert_eq!(record.ts, "2026-06-29T14:00:00Z");
        assert_eq!(
            record.event,
            LedgerEvent::Connection {
                integration: "some-oauth".into(),
                auth: AuthKind::Oauth,
                account: Some("@user".into()),
                scopes,
                expires: Some("2026-07-29T00:00:00Z".into()),
            }
        );
    }

    #[test]
    fn a_connection_without_optionals_reconstructs_empty_and_none() {
        let ev = lns_ocsf::connection(&octx(), "some-oauth", "apikey", None, &[], None);
        let record = ledger_record(&obj(&ev)).unwrap();
        assert_eq!(
            record.event,
            LedgerEvent::Connection {
                integration: "some-oauth".into(),
                auth: AuthKind::Apikey,
                account: None,
                scopes: vec![],
                expires: None,
            }
        );
    }

    #[test]
    fn a_credential_use_reconstructs_and_renders_the_same_detail() {
        let dest = vec!["api.some-provider.example".to_string()];
        let ev =
            lns_ocsf::credential_use(&octx(), "some-provider", "apikey", Some("9c2f1a3d"), &dest);
        let record = ledger_record(&obj(&ev)).unwrap();
        let legacy = LedgerEvent::CredentialUse {
            integration: "some-provider".into(),
            auth: AuthKind::Apikey,
            fp: Some("9c2f1a3d".into()),
            dest,
        };
        assert_eq!(record.event, legacy);
        assert_eq!(events::detail(&record.event), events::detail(&legacy));
        assert_eq!(record.event.name(), "credential_use");
    }

    #[test]
    fn a_credential_use_without_a_destination_reconstructs_empty() {
        let ev = lns_ocsf::credential_use(&octx(), "some-provider", "oauth", None, &[]);
        let record = ledger_record(&obj(&ev)).unwrap();
        assert_eq!(
            record.event,
            LedgerEvent::CredentialUse {
                integration: "some-provider".into(),
                auth: AuthKind::Oauth,
                fp: None,
                dest: vec![],
            }
        );
    }

    #[test]
    fn every_approval_decision_survives_the_round_trip() {
        for (word, decision) in [
            ("allow_once", Decision::AllowOnce),
            ("allow_always", Decision::AllowAlways),
            ("deny_once", Decision::DenyOnce),
            ("deny_always", Decision::DenyAlways),
            ("allow", Decision::Allow),
            ("deny", Decision::Deny),
        ] {
            let ev = lns_ocsf::approval(
                &octx(),
                "network",
                "api.foo.com:443",
                word,
                Some("policy-ambiguous"),
                None,
            );
            let record = ledger_record(&obj(&ev)).unwrap();
            let legacy = LedgerEvent::Approval {
                kind: ApprovalKind::Network,
                target: "api.foo.com:443".into(),
                decision,
                reason: Some("policy-ambiguous".into()),
                integration: None,
            };
            assert_eq!(record.event, legacy, "{word} must round-trip");
            assert_eq!(
                events::detail(&record.event),
                events::detail(&legacy),
                "{word} must render identically to the legacy ledger"
            );
        }
    }

    #[test]
    fn an_approval_reconstructs_the_optional_integration() {
        let ev = lns_ocsf::approval(
            &octx(),
            "credential",
            "some-provider",
            "deny_once",
            None,
            Some("some-provider"),
        );
        let record = ledger_record(&obj(&ev)).unwrap();
        assert_eq!(
            record.event,
            LedgerEvent::Approval {
                kind: ApprovalKind::Credential,
                target: "some-provider".into(),
                decision: Decision::DenyOnce,
                reason: None,
                integration: Some("some-provider".into()),
            }
        );
    }

    fn describe_ocsf(value: &Value) -> (String, String) {
        show::describe(&runlog_obj(&obj(value)).unwrap())
    }

    #[test]
    fn an_egress_reconstructs_to_the_same_run_line_as_the_guest_frame() {
        let ev = lns_ocsf::egress(
            &octx(),
            "GET",
            "http://www.google.com:80/",
            Some(200),
            Some("success"),
            Some("user-allowed-once"),
            true,
        );
        let legacy = legacy_obj(
            r#"{"action":"GET http://www.google.com:80/","status_code":200,"result":"success","metadata":{"reason":"user-allowed-once"},"origin":"guest-proxy"}"#,
        );
        assert_eq!(describe_ocsf(&ev), show::describe(&legacy));
        assert_eq!(
            describe_ocsf(&ev).1,
            "GET www.google.com:80 — allowed once → 200 success"
        );
    }

    #[test]
    fn an_egress_without_a_method_reconstructs_a_bare_url_action() {
        let ev = lns_ocsf::egress(
            &octx(),
            "",
            "api.some-oauth.example:443",
            None,
            None,
            None,
            true,
        );
        let (kind, detail) = describe_ocsf(&ev);
        assert_eq!(kind, "egress");
        assert_eq!(detail, "api.some-oauth.example:443");
    }

    #[test]
    fn an_env_event_reconstructs_the_injected_keys() {
        let mut env = Map::new();
        env.insert("OPENAI_API_KEY".into(), "…".into());
        let ev = lns_ocsf::run_env(&octx(), &env);
        let legacy =
            legacy_obj(r#"{"event":"run_env","env":{"OPENAI_API_KEY":"…"},"origin":"host"}"#);
        assert_eq!(describe_ocsf(&ev), show::describe(&legacy));
        assert_eq!(describe_ocsf(&ev).1, "injected: OPENAI_API_KEY");
    }

    #[test]
    fn a_volume_reconstructs_the_same_run_line() {
        let ev = lns_ocsf::volume_mount(&octx(), "prism-data", "/data");
        let legacy =
            legacy_obj(r#"{"type":"volume_attached","name":"prism-data","target":"/data"}"#);
        assert_eq!(describe_ocsf(&ev), show::describe(&legacy));
        assert_eq!(
            describe_ocsf(&ev),
            ("volume".to_string(), "prism-data → /data".to_string())
        );
    }

    #[test]
    fn a_bind_reconstructs_exposed_secrets_into_the_same_run_line() {
        let ev = lns_ocsf::bind_mount(
            &octx(),
            "/Users/me/proj",
            "/work",
            &[".env".to_string()],
            &[".npmrc".to_string()],
        );
        let legacy = legacy_obj(
            r#"{"type":"bind_attached","source":"/Users/me/proj","target":"/work","exposed_secrets":[".env"],"dropped_secrets":[".npmrc"]}"#,
        );
        assert_eq!(describe_ocsf(&ev), show::describe(&legacy));
        assert_eq!(
            describe_ocsf(&ev).1,
            "/Users/me/proj → /work (exposed: .env)"
        );
    }

    #[test]
    fn the_reconstructed_ts_rides_in_the_run_line_for_the_when_column() {
        let ev = lns_ocsf::volume_mount(&octx(), "data", "/data");
        let legacy = runlog_obj(&obj(&ev)).unwrap();
        assert_eq!(show::when(&legacy), "2026-06-29 14:00:00");
    }

    #[test]
    fn an_event_without_an_unmapped_object_is_rejected() {
        let err = ledger_record(&legacy_obj(r#"{"class_uid":3002}"#)).unwrap_err();
        assert!(format!("{err:#}").contains("no unmapped object"), "{err:#}");
    }

    #[test]
    fn a_ledger_kind_is_rejected_by_the_run_reader_and_vice_versa() {
        let egress = lns_ocsf::egress(&octx(), "GET", "http://x/", None, None, None, true);
        let ledger_err = ledger_record(&obj(&egress)).unwrap_err();
        assert!(
            format!("{ledger_err:#}").contains("not a ledger kind"),
            "{ledger_err:#}"
        );

        let connection = lns_ocsf::connection(&octx(), "some-oauth", "oauth", None, &[], None);
        let run_err = runlog_obj(&obj(&connection)).unwrap_err();
        assert!(
            format!("{run_err:#}").contains("not a per-run kind"),
            "{run_err:#}"
        );
    }

    #[test]
    fn a_missing_required_field_is_surfaced_with_its_key() {
        let err = ledger_record(&legacy_obj(
            r#"{"class_uid":3002,"unmapped":{"lns_kind":"connection","lns_run":"r","lns_microvm":"m","lns_ts":"t","lns_auth":"oauth"}}"#,
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("lns_integration"), "{err:#}");
    }

    #[test]
    fn an_unparseable_enum_value_is_surfaced() {
        let err = ledger_record(&legacy_obj(
            r#"{"class_uid":3002,"unmapped":{"lns_kind":"connection","lns_run":"r","lns_microvm":"m","lns_ts":"t","lns_integration":"i","lns_auth":"telepathy"}}"#,
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("lns_auth"), "{err:#}");
    }

    #[test]
    fn an_egress_without_a_url_is_rejected() {
        let err = runlog_obj(&legacy_obj(
            r#"{"class_uid":4002,"unmapped":{"lns_kind":"egress","lns_run":"r","lns_microvm":"m","lns_ts":"t"}}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("http_request.url.text"),
            "{err:#}"
        );
    }

    #[test]
    fn an_env_event_without_its_env_map_is_rejected() {
        let err = runlog_obj(&legacy_obj(
            r#"{"class_uid":1007,"unmapped":{"lns_kind":"env","lns_run":"r","lns_microvm":"m","lns_ts":"t"}}"#,
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("lns_env"), "{err:#}");
    }
}
