use serde_json::{Map, Value, json};

use crate::base::{
    Context, Event, activity, category, class, device_type, disposition, file_type,
    http_method_activity, severity, status,
};

fn auth_protocol_id(auth: &str) -> u8 {
    if auth == "oauth" { 6 } else { 99 }
}

fn microvm_device(ctx: &Context) -> Value {
    json!({"type_id": device_type::VIRTUAL, "name": ctx.microvm})
}

fn lns_actor() -> Value {
    json!({"app_name": "lns"})
}

pub fn connection(
    ctx: &Context,
    integration: &str,
    auth: &str,
    account: Option<&str>,
    scopes: &[String],
    expires: Option<&str>,
) -> Value {
    let mut ev = Event::new(
        "connection",
        class::AUTHENTICATION,
        category::IAM,
        activity::LOGON,
        severity::INFORMATIONAL,
        ctx,
    )
    .set("user", json!({"name": account.unwrap_or(integration)}))
    .set("service", json!({"name": integration}))
    .set("auth_protocol_id", auth_protocol_id(auth).into())
    .note("lns_integration", integration.into())
    .note("lns_auth", auth.into());
    if let Some(account) = account {
        ev = ev.note("lns_account", account.into());
    }
    if !scopes.is_empty() {
        ev = ev.note("lns_scopes", json!(scopes));
    }
    if let Some(expires) = expires {
        ev = ev.note("lns_expires", expires.into());
    }
    ev.build()
}

pub fn credential_use(
    ctx: &Context,
    integration: &str,
    auth: &str,
    fingerprint: Option<&str>,
    dest: &[String],
) -> Value {
    let mut ev = Event::new(
        "credential",
        class::AUTHENTICATION,
        category::IAM,
        activity::LOGON,
        severity::INFORMATIONAL,
        ctx,
    )
    .set("user", json!({"name": integration}))
    .set("auth_protocol_id", auth_protocol_id(auth).into())
    .note("lns_integration", integration.into())
    .note("lns_auth", auth.into());
    match dest.first() {
        Some(first) => ev = ev.set("dst_endpoint", json!({"domain": first})),
        None => ev = ev.set("service", json!({"name": integration})),
    }
    if let Some(fingerprint) = fingerprint {
        ev = ev.note("lns_fp", fingerprint.into());
    }
    if !dest.is_empty() {
        ev = ev.note("lns_dest", json!(dest));
    }
    ev.build()
}

pub fn approval(
    ctx: &Context,
    approval_kind: &str,
    target: &str,
    decision: &str,
    reason: Option<&str>,
    integration: Option<&str>,
) -> Value {
    let allowed = decision.starts_with("allow");
    let sev = if allowed {
        severity::INFORMATIONAL
    } else {
        severity::MEDIUM
    };
    let disp = if allowed {
        disposition::ALLOWED
    } else {
        disposition::BLOCKED
    };
    let mut ev = Event::new(
        "approval",
        class::DETECTION_FINDING,
        category::FINDINGS,
        activity::FINDING_CREATE,
        sev,
        ctx,
    )
    .set(
        "finding_info",
        json!({"uid": target, "title": reason.unwrap_or(approval_kind)}),
    )
    .set("disposition_id", disp.into())
    .note("lns_approval_kind", approval_kind.into())
    .note("lns_decision", decision.into())
    .note("lns_target", target.into());
    if let Some(reason) = reason {
        ev = ev.note("lns_reason", reason.into());
    }
    if let Some(integration) = integration {
        ev = ev.note("lns_integration", integration.into());
    }
    ev.build()
}

pub fn egress(
    ctx: &Context,
    method: &str,
    url: &str,
    status_code: Option<u64>,
    result: Option<&str>,
    reason: Option<&str>,
    guest_proxied: bool,
) -> Value {
    let mut ev = Event::new(
        "egress",
        class::HTTP_ACTIVITY,
        category::NETWORK,
        http_method_activity(method),
        severity::INFORMATIONAL,
        ctx,
    )
    .set(
        "http_request",
        json!({"http_method": method, "url": {"text": url}}),
    )
    .note(
        "lns_origin",
        if guest_proxied { "guest-proxy" } else { "host" }.into(),
    );
    if let Some(code) = status_code {
        ev = ev.set("http_response", json!({"code": code}));
    }
    if let Some(result) = result {
        let id = if result == "success" {
            status::SUCCESS
        } else {
            status::FAILURE
        };
        ev = ev.set("status_id", id.into());
        ev = ev.note("lns_result", result.into());
    }
    if let Some(reason) = reason {
        ev = ev.note("lns_reason", reason.into());
        if reason.contains("allowed") {
            ev = ev.set("disposition_id", disposition::ALLOWED.into());
        } else if reason.contains("denied") {
            ev = ev.set("disposition_id", disposition::BLOCKED.into());
        }
    }
    ev.build()
}

pub fn run_env(ctx: &Context, env: &Map<String, Value>) -> Value {
    Event::new(
        "env",
        class::PROCESS_ACTIVITY,
        category::SYSTEM,
        activity::PROCESS_LAUNCH,
        severity::INFORMATIONAL,
        ctx,
    )
    .set("process", json!({"uid": ctx.run, "name": "workload"}))
    .set("device", microvm_device(ctx))
    .set("actor", lns_actor())
    .note("lns_origin", "host".into())
    .note("lns_env", Value::Object(env.clone()))
    .build()
}

pub fn volume_mount(ctx: &Context, name: &str, target: &str) -> Value {
    Event::new(
        "volume",
        class::FILE_ACTIVITY,
        category::SYSTEM,
        activity::FILE_MOUNT,
        severity::INFORMATIONAL,
        ctx,
    )
    .set(
        "file",
        json!({"name": target, "type_id": file_type::FOLDER}),
    )
    .set("device", microvm_device(ctx))
    .set("actor", lns_actor())
    .note("lns_origin", "host".into())
    .note("lns_name", name.into())
    .note("lns_target", target.into())
    .build()
}

pub fn bind_mount(
    ctx: &Context,
    source: &str,
    target: &str,
    exposed_secrets: &[String],
    dropped_secrets: &[String],
) -> Value {
    let sev = if exposed_secrets.is_empty() {
        severity::INFORMATIONAL
    } else {
        severity::MEDIUM
    };
    let mut ev = Event::new(
        "bind",
        class::FILE_ACTIVITY,
        category::SYSTEM,
        activity::FILE_MOUNT,
        sev,
        ctx,
    )
    .set(
        "file",
        json!({"name": target, "type_id": file_type::FOLDER}),
    )
    .set("device", microvm_device(ctx))
    .set("actor", lns_actor())
    .note("lns_origin", "host".into())
    .note("lns_source", source.into())
    .note("lns_target", target.into());
    if !exposed_secrets.is_empty() {
        ev = ev.note("lns_exposed_secrets", json!(exposed_secrets));
    }
    if !dropped_secrets.is_empty() {
        ev = ev.note("lns_dropped_secrets", json!(dropped_secrets));
    }
    ev.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::assert_schema_valid;

    fn ctx() -> Context<'static> {
        Context {
            time_unix_secs: 1_780_000_000,
            ts_rfc3339: "2026-06-29T14:00:00Z",
            run: "9e8d7c6b0000",
            microvm: "calm-finch",
        }
    }

    #[test]
    fn connection_is_an_authentication_with_a_user_and_service() {
        let ev = connection(
            &ctx(),
            "some-oauth",
            "oauth",
            Some("@user"),
            &["repo".into(), "read:org".into()],
            Some("2026-07-29T00:00:00Z"),
        );
        assert_schema_valid(&ev);
        assert_eq!(ev["class_uid"], 3002);
        assert_eq!(ev["user"]["name"], "@user");
        assert_eq!(ev["service"]["name"], "some-oauth");
        assert_eq!(ev["auth_protocol_id"], 6);
        assert_eq!(ev["unmapped"]["lns_kind"], "connection");
        assert_eq!(ev["unmapped"]["lns_account"], "@user");
        assert_eq!(ev["unmapped"]["lns_scopes"][1], "read:org");
        assert_eq!(ev["unmapped"]["lns_expires"], "2026-07-29T00:00:00Z");
    }

    #[test]
    fn connection_without_an_account_names_the_user_after_the_integration() {
        let ev = connection(&ctx(), "some-oauth", "apikey", None, &[], None);
        assert_schema_valid(&ev);
        assert_eq!(ev["user"]["name"], "some-oauth");
        assert_eq!(ev["auth_protocol_id"], 99);
        assert!(ev["unmapped"].get("lns_account").is_none());
        assert!(ev["unmapped"].get("lns_scopes").is_none());
        assert!(ev["unmapped"].get("lns_expires").is_none());
    }

    #[test]
    fn credential_use_targets_a_destination_endpoint() {
        let ev = credential_use(
            &ctx(),
            "some-provider",
            "apikey",
            Some("9c2f1a3d"),
            &["api.some-provider.example".into()],
        );
        assert_schema_valid(&ev);
        assert_eq!(ev["unmapped"]["lns_kind"], "credential");
        assert_eq!(ev["dst_endpoint"]["domain"], "api.some-provider.example");
        assert_eq!(ev["unmapped"]["lns_fp"], "9c2f1a3d");
        assert_eq!(ev["unmapped"]["lns_dest"][0], "api.some-provider.example");
    }

    #[test]
    fn credential_use_with_no_destination_falls_back_to_a_service() {
        let ev = credential_use(&ctx(), "some-provider", "oauth", None, &[]);
        assert_schema_valid(&ev);
        assert_eq!(ev["service"]["name"], "some-provider");
        assert!(ev.get("dst_endpoint").is_none());
        assert!(ev["unmapped"].get("lns_fp").is_none());
        assert!(ev["unmapped"].get("lns_dest").is_none());
    }

    #[test]
    fn an_allowed_approval_is_an_informational_finding_with_allowed_disposition() {
        let ev = approval(
            &ctx(),
            "network",
            "api.example.test:443",
            "allow_always",
            Some("policy-ambiguous"),
            None,
        );
        assert_schema_valid(&ev);
        assert_eq!(ev["class_uid"], 2004);
        assert_eq!(ev["severity_id"], 1);
        assert_eq!(ev["disposition_id"], 1);
        assert_eq!(ev["finding_info"]["uid"], "api.example.test:443");
        assert_eq!(ev["finding_info"]["title"], "policy-ambiguous");
        assert_eq!(ev["unmapped"]["lns_approval_kind"], "network");
        assert_eq!(
            ev["unmapped"]["lns_decision"], "allow_always",
            "the exact decision survives so the timeline still reads allow-always, not a collapsed allow"
        );
        assert_eq!(ev["unmapped"]["lns_reason"], "policy-ambiguous");
    }

    #[test]
    fn a_denied_approval_raises_severity_and_blocks_and_titles_after_the_kind() {
        let ev = approval(
            &ctx(),
            "credential",
            "some-provider",
            "deny_always",
            None,
            Some("some-provider"),
        );
        assert_schema_valid(&ev);
        assert_eq!(ev["severity_id"], 3);
        assert_eq!(ev["disposition_id"], 2);
        assert_eq!(ev["finding_info"]["title"], "credential");
        assert_eq!(ev["unmapped"]["lns_decision"], "deny_always");
        assert_eq!(ev["unmapped"]["lns_integration"], "some-provider");
        assert!(ev["unmapped"].get("lns_reason").is_none());
    }

    #[test]
    fn egress_is_an_http_activity_with_request_response_and_disposition() {
        let ev = egress(
            &ctx(),
            "GET",
            "http://api.example.test:443/",
            Some(200),
            Some("success"),
            Some("user-allowed-once"),
            true,
        );
        assert_schema_valid(&ev);
        assert_eq!(ev["class_uid"], 4002);
        assert_eq!(ev["activity_id"], 3);
        assert_eq!(ev["http_request"]["http_method"], "GET");
        assert_eq!(
            ev["http_request"]["url"]["text"],
            "http://api.example.test:443/"
        );
        assert_eq!(ev["http_response"]["code"], 200);
        assert_eq!(ev["status_id"], 1);
        assert_eq!(ev["unmapped"]["lns_result"], "success");
        assert_eq!(ev["disposition_id"], 1);
        assert_eq!(ev["unmapped"]["lns_origin"], "guest-proxy");
    }

    #[test]
    fn a_failed_denied_egress_maps_status_and_disposition() {
        let ev = egress(
            &ctx(),
            "POST",
            "http://x.test/",
            None,
            Some("error"),
            Some("user-denied-once"),
            false,
        );
        assert_schema_valid(&ev);
        assert_eq!(ev["activity_id"], 6);
        assert_eq!(ev["status_id"], 2);
        assert_eq!(
            ev["unmapped"]["lns_result"], "error",
            "a non-success result is preserved verbatim, not flattened to a boolean"
        );
        assert_eq!(ev["disposition_id"], 2);
        assert_eq!(ev["unmapped"]["lns_origin"], "host");
        assert!(ev.get("http_response").is_none());
    }

    #[test]
    fn an_egress_without_status_or_recognised_reason_omits_them() {
        let ev = egress(
            &ctx(),
            "GET",
            "http://x.test/",
            None,
            None,
            Some("prefetch"),
            true,
        );
        assert_schema_valid(&ev);
        assert!(ev.get("status_id").is_none());
        assert!(ev["unmapped"].get("lns_result").is_none());
        assert!(ev.get("disposition_id").is_none());
        assert_eq!(ev["unmapped"]["lns_reason"], "prefetch");
    }

    #[test]
    fn run_env_is_a_process_activity_with_a_device_and_actor() {
        let mut env = Map::new();
        env.insert("OPENAI_API_KEY".into(), "…".into());
        env.insert("PATH".into(), "…".into());
        let ev = run_env(&ctx(), &env);
        assert_schema_valid(&ev);
        assert_eq!(ev["class_uid"], 1007);
        assert_eq!(ev["process"]["uid"], "9e8d7c6b0000");
        assert_eq!(ev["device"]["type_id"], 6);
        assert_eq!(ev["device"]["name"], "calm-finch");
        assert_eq!(ev["actor"]["app_name"], "lns");
        assert!(ev["unmapped"]["lns_env"]["OPENAI_API_KEY"].is_string());
    }

    #[test]
    fn a_volume_mount_is_a_file_activity_on_a_folder() {
        let ev = volume_mount(&ctx(), "data", "/data");
        assert_schema_valid(&ev);
        assert_eq!(ev["class_uid"], 1001);
        assert_eq!(ev["activity_id"], 12);
        assert_eq!(ev["file"]["name"], "/data");
        assert_eq!(ev["file"]["type_id"], 2);
        assert_eq!(ev["unmapped"]["lns_name"], "data");
        assert_eq!(ev["unmapped"]["lns_target"], "/data");
    }

    #[test]
    fn a_bind_that_exposes_a_secret_raises_severity() {
        let ev = bind_mount(
            &ctx(),
            "/Users/me/proj",
            "/work",
            &[".env".into()],
            &[".npmrc".into(), ".ssh".into()],
        );
        assert_schema_valid(&ev);
        assert_eq!(ev["severity_id"], 3);
        assert_eq!(ev["unmapped"]["lns_source"], "/Users/me/proj");
        assert_eq!(ev["unmapped"]["lns_exposed_secrets"][0], ".env");
        assert_eq!(ev["unmapped"]["lns_dropped_secrets"][1], ".ssh");
    }

    #[test]
    fn a_bind_with_no_exposed_secret_stays_informational() {
        let ev = bind_mount(&ctx(), "/src", "/work", &[], &[]);
        assert_schema_valid(&ev);
        assert_eq!(ev["severity_id"], 1);
        assert!(ev["unmapped"].get("lns_exposed_secrets").is_none());
        assert!(ev["unmapped"].get("lns_dropped_secrets").is_none());
    }
}
