use lns_ipc::{ApprovalKind, AuthKind, Decision, LedgerEvent, LedgerRecord};
use serde_json::{Map, Value};

use crate::time_fmt::{rfc3339_from_unix, unix_from_rfc3339};

pub struct OcsfCtx {
    unix_secs: u64,
    ts: String,
    run: String,
    microvm: String,
}

impl OcsfCtx {
    pub fn at_unix(run: String, microvm: String, unix_secs: u64) -> Self {
        Self {
            unix_secs,
            ts: rfc3339_from_unix(unix_secs),
            run,
            microvm,
        }
    }

    fn at_ts(run: String, microvm: String, ts: String) -> Self {
        Self {
            unix_secs: unix_from_rfc3339(&ts),
            ts,
            run,
            microvm,
        }
    }

    fn ctx(&self) -> lns_ocsf::Context<'_> {
        lns_ocsf::Context {
            time_unix_secs: self.unix_secs,
            ts_rfc3339: &self.ts,
            run: &self.run,
            microvm: &self.microvm,
        }
    }
}

pub fn ledger_event(record: &LedgerRecord) -> Map<String, Value> {
    let cx = OcsfCtx::at_ts(
        record.run.clone(),
        record.microvm.clone(),
        record.ts.clone(),
    );
    let value = match &record.event {
        LedgerEvent::Connection {
            integration,
            auth,
            account,
            scopes,
            expires,
        } => lns_ocsf::connection(
            &cx.ctx(),
            integration,
            auth_word(*auth),
            account.as_deref(),
            scopes,
            expires.as_deref(),
        ),
        LedgerEvent::CredentialUse {
            integration,
            auth,
            fp,
            dest,
        } => lns_ocsf::credential_use(
            &cx.ctx(),
            integration,
            auth_word(*auth),
            fp.as_deref(),
            dest,
        ),
        LedgerEvent::Approval {
            kind,
            target,
            decision,
            reason,
            integration,
        } => lns_ocsf::approval(
            &cx.ctx(),
            approval_kind_word(*kind),
            target,
            decision_word(*decision),
            reason.as_deref(),
            integration.as_deref(),
        ),
    };
    into_object(value)
}

pub fn run_env_event(cx: &OcsfCtx, env: &Map<String, Value>) -> Map<String, Value> {
    into_object(lns_ocsf::run_env(&cx.ctx(), env))
}

pub fn workload_launch_event(cx: &OcsfCtx, image: &str) -> Map<String, Value> {
    into_object(lns_ocsf::workload_launch(&cx.ctx(), image))
}

pub fn volume_event(cx: &OcsfCtx, name: &str, target: &str) -> Map<String, Value> {
    into_object(lns_ocsf::volume_mount(&cx.ctx(), name, target))
}

pub fn bind_event(
    cx: &OcsfCtx,
    source: &str,
    target: &str,
    exposed_secrets: &[String],
    dropped_secrets: &[String],
) -> Map<String, Value> {
    into_object(lns_ocsf::bind_mount(
        &cx.ctx(),
        source,
        target,
        exposed_secrets,
        dropped_secrets,
    ))
}

pub fn egress_event(
    cx: &OcsfCtx,
    method: &str,
    url: &str,
    status_code: Option<u64>,
    result: Option<&str>,
    reason: Option<&str>,
    guest_proxied: bool,
) -> Map<String, Value> {
    into_object(lns_ocsf::egress(
        &cx.ctx(),
        method,
        url,
        status_code,
        result,
        reason,
        guest_proxied,
    ))
}

fn into_object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(obj) => obj,
        _ => Map::new(),
    }
}

fn auth_word(auth: AuthKind) -> &'static str {
    match auth {
        AuthKind::Oauth => "oauth",
        AuthKind::Apikey => "apikey",
    }
}

fn approval_kind_word(kind: ApprovalKind) -> &'static str {
    match kind {
        ApprovalKind::Network => "network",
        ApprovalKind::Credential => "credential",
        ApprovalKind::Integration => "integration",
    }
}

fn decision_word(decision: Decision) -> &'static str {
    match decision {
        Decision::AllowOnce => "allow_once",
        Decision::AllowAlways => "allow_always",
        Decision::DenyOnce => "deny_once",
        Decision::DenyAlways => "deny_always",
        Decision::Allow => "allow",
        Decision::Deny => "deny",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(event: LedgerEvent) -> LedgerRecord {
        LedgerRecord {
            ts: "2026-06-29T14:00:00Z".into(),
            run: "9e8d7c6b0000".into(),
            microvm: "calm-finch".into(),
            event,
        }
    }

    #[test]
    fn decision_word_matches_the_serde_wire_form_the_reader_parses() {
        for d in [
            Decision::AllowOnce,
            Decision::AllowAlways,
            Decision::DenyOnce,
            Decision::DenyAlways,
            Decision::Allow,
            Decision::Deny,
        ] {
            let wire = serde_json::to_value(d).unwrap();
            assert_eq!(
                Value::String(decision_word(d).to_string()),
                wire,
                "decision_word must equal the serde form so lns-audit can parse it back"
            );
        }
    }

    #[test]
    fn a_connection_record_becomes_an_authentication_event() {
        let ev = ledger_event(&record(LedgerEvent::Connection {
            integration: "some-oauth".into(),
            auth: AuthKind::Oauth,
            account: Some("@user".into()),
            scopes: vec!["repo".into()],
            expires: None,
        }));
        assert_eq!(ev["class_uid"], 3002);
        assert_eq!(ev["unmapped"]["lns_kind"], "connection");
        assert_eq!(ev["unmapped"]["lns_auth"], "oauth");
        assert_eq!(ev["unmapped"]["lns_run"], "9e8d7c6b0000");
        assert_eq!(
            ev["time"],
            unix_from_rfc3339("2026-06-29T14:00:00Z") as i64 * 1000,
            "OCSF time is derived from the record's rfc3339 stamp"
        );
    }

    #[test]
    fn a_credential_record_carries_the_apikey_auth_word() {
        let ev = ledger_event(&record(LedgerEvent::CredentialUse {
            integration: "some-provider".into(),
            auth: AuthKind::Apikey,
            fp: Some("9c2f1a3d".into()),
            dest: vec!["api.some-provider.example".into()],
        }));
        assert_eq!(ev["unmapped"]["lns_kind"], "credential");
        assert_eq!(ev["unmapped"]["lns_auth"], "apikey");
    }

    #[test]
    fn an_approval_record_records_the_exact_decision_and_kind() {
        let ev = ledger_event(&record(LedgerEvent::Approval {
            kind: ApprovalKind::Integration,
            target: "some-oauth".into(),
            decision: Decision::DenyAlways,
            reason: None,
            integration: Some("some-oauth".into()),
        }));
        assert_eq!(ev["class_uid"], 2004);
        assert_eq!(ev["unmapped"]["lns_approval_kind"], "integration");
        assert_eq!(ev["unmapped"]["lns_decision"], "deny_always");
        assert_eq!(ev["disposition_id"], 2, "deny blocks");
    }

    #[test]
    fn each_approval_kind_maps_to_its_word() {
        for (kind, word) in [
            (ApprovalKind::Network, "network"),
            (ApprovalKind::Credential, "credential"),
            (ApprovalKind::Integration, "integration"),
        ] {
            let ev = ledger_event(&record(LedgerEvent::Approval {
                kind,
                target: "t".into(),
                decision: Decision::AllowOnce,
                reason: None,
                integration: None,
            }));
            assert_eq!(ev["unmapped"]["lns_approval_kind"], word);
        }
    }

    #[test]
    fn per_run_events_carry_the_microvm_device_and_clock_time() {
        let cx = OcsfCtx::at_unix("9e8d7c6b0000".into(), "calm-finch".into(), 1_780_000_000);

        let mut env = Map::new();
        env.insert("OPENAI_API_KEY".into(), "…".into());
        let run_env = run_env_event(&cx, &env);
        assert_eq!(run_env["class_uid"], 1007);
        assert_eq!(run_env["device"]["name"], "calm-finch");
        assert_eq!(run_env["time"], 1_780_000_000_000i64);

        let volume = volume_event(&cx, "data", "/data");
        assert_eq!(volume["unmapped"]["lns_kind"], "volume");
        assert_eq!(volume["file"]["name"], "/data");

        let bind = bind_event(&cx, "/src", "/work", &[".env".into()], &[]);
        assert_eq!(bind["unmapped"]["lns_kind"], "bind");
        assert_eq!(bind["severity_id"], 3, "an exposed secret raises severity");

        let egress = egress_event(
            &cx,
            "GET",
            "http://x/",
            Some(200),
            Some("success"),
            None,
            true,
        );
        assert_eq!(egress["class_uid"], 4002);
        assert_eq!(egress["unmapped"]["lns_result"], "success");
        assert_eq!(egress["unmapped"]["lns_origin"], "guest-proxy");
    }

    #[test]
    fn into_object_defaults_a_non_object_to_empty() {
        assert!(into_object(Value::from(7)).is_empty());
    }
}
