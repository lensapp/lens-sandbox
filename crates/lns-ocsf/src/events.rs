use serde_json::{Map, Value, json};

use crate::base::{
    Context, Event, activity, category, class, device_type, disposition, file_type,
    http_method_activity, severity, status,
};

fn microvm_device(ctx: &Context) -> Value {
    json!({"type_id": device_type::VIRTUAL, "name": ctx.microvm})
}

fn lns_actor() -> Value {
    json!({"app_name": "lns"})
}

fn decision_word(decision: &str) -> String {
    decision.replace('_', "-")
}

fn request_summary(method: &str, url: &str) -> String {
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

fn reason_phrase(reason: &str) -> &str {
    match reason {
        "user-allowed-once" => "allowed once",
        "user-allowed-persisted" => "allowed always",
        "user-denied-once" => "denied once",
        "user-denied-persisted" => "denied always",
        "policy-ambiguous" => "needs your decision",
        "policy-deny" => "blocked by policy",
        other => other,
    }
}

fn egress_outcome(status_code: Option<u64>, result: Option<&str>) -> Option<String> {
    match (status_code, result) {
        (Some(code), Some(result)) => Some(format!("→ {code} {result}")),
        (Some(code), None) => Some(format!("→ {code}")),
        (None, Some(result)) => Some(format!("→ {result}")),
        (None, None) => None,
    }
}

fn dst_endpoint(url: &str) -> Value {
    let host = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(url);
    match host.rsplit_once(':') {
        Some((domain, port)) if port.parse::<u64>().is_ok() => {
            json!({"domain": domain, "port": port.parse::<u64>().unwrap_or_default()})
        }
        _ => json!({"domain": host}),
    }
}

pub fn approval(
    ctx: &Context,
    approval_kind: &str,
    target: &str,
    decision: &str,
    reason: Option<&str>,
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
    let base = format!("{approval_kind} {} {target}", decision_word(decision));
    let message = match reason {
        Some(reason) => format!("{base}  [{}]", reason_phrase(reason)),
        None => base,
    };
    let mut ev = Event::new(
        "approval",
        class::DETECTION_FINDING,
        category::FINDINGS,
        activity::FINDING_CREATE,
        sev,
        ctx,
    )
    .set("message", message.into())
    .set(
        "finding_info",
        json!({"uid": target, "title": reason.map(reason_phrase).unwrap_or(approval_kind)}),
    )
    .set_disposition(disp)
    .note("lns_approval_kind", approval_kind.into())
    .note("lns_decision", decision.into())
    .note("lns_target", target.into());
    if let Some(reason) = reason {
        ev = ev.note("lns_reason", reason.into());
    }
    ev.build()
}

/// One run's decision about one connector, as `lns audit --kind connector` reads it. The digest a grant bound to is disclosed beside the event and not in the message, the way `sandbox_run` treats its own.
pub fn connector(
    ctx: &Context,
    connector: &str,
    verb: &str,
    method: Option<&str>,
    connection: Option<&str>,
    digest: Option<&str>,
    answered_by: Option<&str>,
) -> Value {
    let mut ev = Event::new(
        "connector",
        class::AUTHENTICATION,
        category::IAM,
        activity::LOGON,
        severity::INFORMATIONAL,
        ctx,
    )
    .set(
        "message",
        connector_message(connector, verb, method, connection).into(),
    )
    .set("service", json!({"name": connector}))
    .set("actor", lns_actor())
    .note("lns_connector", connector.into())
    .note("lns_verb", verb.into());
    if let Some(method) = method {
        ev = ev.note("lns_method", method.into());
    }
    if let Some(connection) = connection {
        ev = ev.note("lns_connection", connection.into());
    }
    if let Some(digest) = digest {
        ev = ev.note("lns_connector_digest", digest.into());
    }
    if let Some(answered_by) = answered_by {
        ev = ev.note("lns_answer_source", answered_by.into());
    }
    ev.build()
}

fn connector_message(
    connector: &str,
    verb: &str,
    method: Option<&str>,
    connection: Option<&str>,
) -> String {
    match (method, connection) {
        (Some(method), Some(connection)) => format!("{verb} {connector} {method} as {connection}"),
        (Some(method), None) => format!("{verb} {connector} {method}"),
        _ => format!("{verb} {connector}"),
    }
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
    let mut message = request_summary(method, url);
    if let Some(reason) = reason {
        message.push_str(&format!(" — {}", reason_phrase(reason)));
    }
    if let Some(outcome) = egress_outcome(status_code, result) {
        message.push(' ');
        message.push_str(&outcome);
    }
    let mut ev = Event::new(
        "egress",
        class::HTTP_ACTIVITY,
        category::NETWORK,
        http_method_activity(method),
        severity::INFORMATIONAL,
        ctx,
    )
    .set("message", message.into())
    .set(
        "http_request",
        json!({"http_method": method, "url": {"text": url}}),
    )
    .set("dst_endpoint", dst_endpoint(url))
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
        ev = ev.set_status(id);
        ev = ev.note("lns_result", result.into());
    }
    if let Some(reason) = reason {
        ev = ev
            .set("status_detail", reason_phrase(reason).into())
            .note("lns_reason", reason.into());
        if reason.contains("allowed") {
            ev = ev.set_disposition(disposition::ALLOWED);
        } else if reason.contains("denied") {
            ev = ev.set_disposition(disposition::BLOCKED);
        }
    }
    ev.build()
}

pub fn workload_launch(ctx: &Context, image: &str) -> Value {
    Event::new(
        "launch",
        class::PROCESS_ACTIVITY,
        category::SYSTEM,
        activity::PROCESS_LAUNCH,
        severity::INFORMATIONAL,
        ctx,
    )
    .set("message", format!("launched {image}").into())
    .set("process", json!({"uid": ctx.run, "name": "workload"}))
    .set("device", microvm_device(ctx))
    .set("actor", lns_actor())
    .note("lns_origin", "host".into())
    .note("lns_image", image.into())
    .build()
}

pub fn workload_exit(ctx: &Context, exit_code: i32, killed: bool) -> Value {
    let how = if killed { "killed" } else { "exited" };
    Event::new(
        "exit",
        class::PROCESS_ACTIVITY,
        category::SYSTEM,
        activity::PROCESS_TERMINATE,
        severity::INFORMATIONAL,
        ctx,
    )
    .set(
        "message",
        format!("workload {how} with code {exit_code}").into(),
    )
    .set("process", json!({"uid": ctx.run, "name": "workload"}))
    .set("device", microvm_device(ctx))
    .set("actor", lns_actor())
    .note("lns_origin", "host".into())
    .note("lns_exit_code", exit_code.into())
    .note("lns_killed", killed.into())
    .build()
}

pub fn network_setup_failed(ctx: &Context, exit_code: i32, error: &str) -> Value {
    broker_refusal(
        ctx,
        "network_setup_failed",
        format!("the guest could not set up its network: {error}"),
        exit_code,
    )
}

pub fn no_dhcp_lease(ctx: &Context, exit_code: i32) -> Value {
    broker_refusal(
        ctx,
        "no_dhcp_lease",
        "the guest got no DHCP lease from the host network".to_string(),
        exit_code,
    )
}

fn broker_refusal(ctx: &Context, kind: &'static str, message: String, exit_code: i32) -> Value {
    Event::new(
        kind,
        class::PROCESS_ACTIVITY,
        category::SYSTEM,
        activity::PROCESS_TERMINATE,
        severity::MEDIUM,
        ctx,
    )
    .set("message", message.into())
    .set("process", json!({"uid": ctx.run, "name": "session-broker"}))
    .set("device", microvm_device(ctx))
    .set("actor", lns_actor())
    .set_status(status::FAILURE)
    .note("lns_origin", "guest".into())
    .note("lns_exit_code", exit_code.into())
    .build()
}

pub fn workload_restart(ctx: &Context, image: &str) -> Value {
    Event::new(
        "restart",
        class::PROCESS_ACTIVITY,
        category::SYSTEM,
        activity::PROCESS_LAUNCH,
        severity::INFORMATIONAL,
        ctx,
    )
    .set(
        "message",
        format!("restarted {image} on its preserved state; policy re-resolved live").into(),
    )
    .set("process", json!({"uid": ctx.run, "name": "workload"}))
    .set("device", microvm_device(ctx))
    .set("actor", lns_actor())
    .note("lns_origin", "host".into())
    .note("lns_image", image.into())
    .build()
}

pub fn run_removed(ctx: &Context, forced: bool, auto: bool) -> Value {
    let how = if forced {
        "forced (lns rm -f)"
    } else if auto {
        "auto (--rm)"
    } else {
        "requested (lns rm)"
    };
    Event::new(
        "run_removed",
        class::FILE_ACTIVITY,
        category::SYSTEM,
        activity::FILE_DELETE,
        severity::INFORMATIONAL,
        ctx,
    )
    .set(
        "message",
        format!(
            "run removed, {how}: its record and writable layer are gone; this log outlives them"
        )
        .into(),
    )
    .set("file", json!({"name": ctx.run, "type_id": 2}))
    .set("device", microvm_device(ctx))
    .set("actor", lns_actor())
    .note("lns_origin", "host".into())
    .note("lns_forced", forced.into())
    .note("lns_auto", auto.into())
    .build()
}

pub fn runs_pruned(ctx: &Context, removed: &[String]) -> Value {
    Event::new(
        "runs_pruned",
        class::FILE_ACTIVITY,
        category::SYSTEM,
        activity::FILE_DELETE,
        severity::INFORMATIONAL,
        ctx,
    )
    .set(
        "message",
        format!("prune swept stopped runs: {}", removed.join(", ")).into(),
    )
    .set("file", json!({"name": ctx.run, "type_id": 2}))
    .set("device", microvm_device(ctx))
    .set("actor", lns_actor())
    .note("lns_origin", "host".into())
    .note(
        "lns_removed",
        Value::Array(removed.iter().cloned().map(Value::String).collect()),
    )
    .build()
}

pub fn run_env(ctx: &Context, env: &Map<String, Value>) -> Value {
    let message = format!(
        "injected: {}",
        env.keys().cloned().collect::<Vec<_>>().join(", ")
    );
    Event::new(
        "env",
        class::PROCESS_ACTIVITY,
        category::SYSTEM,
        activity::PROCESS_LAUNCH,
        severity::INFORMATIONAL,
        ctx,
    )
    .set("message", message.into())
    .set("process", json!({"uid": ctx.run, "name": "workload"}))
    .set("device", microvm_device(ctx))
    .set("actor", lns_actor())
    .note("lns_origin", "host".into())
    .note("lns_env", Value::Object(env.clone()))
    .build()
}

pub fn tool_provision(
    ctx: &Context,
    tool: &str,
    requested: &str,
    resolved: &str,
    source_host: Option<&str>,
    backend: &str,
) -> Value {
    // Acquisition creates a tree in the host cache; nothing is mounted yet, and a SIEM rule on Mount would read a download as a filesystem mount.
    let mut event = Event::new(
        "tool",
        class::FILE_ACTIVITY,
        category::SYSTEM,
        activity::FILE_CREATE,
        severity::INFORMATIONAL,
        ctx,
    )
    .set(
        "message",
        match source_host {
            Some(host) => format!("provisioned {requested} → {resolved} from {host}"),
            None => format!("provisioned {requested} → {resolved}"),
        }
        .into(),
    )
    .set(
        "file",
        json!({"name": format!("/.lens/tools/{tool}/{resolved}"), "type_id": file_type::FOLDER}),
    )
    .set("actor", lns_actor())
    .note("lns_origin", "host".into())
    .note("lns_tool", tool.into())
    .note("lns_requested", requested.into())
    .note("lns_resolved", resolved.into())
    .note("lns_backend", backend.into());
    // A guessed host is a false attestation; the backend reference is what we actually know.
    if let Some(host) = source_host {
        event = event.note("lns_source", host.into());
    }
    // A pull provisions before any microVM exists, so there is no device to name.
    if !ctx.microvm.is_empty() {
        event = event.set("device", microvm_device(ctx));
    }
    event.build()
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
    .set("message", format!("{name} → {target}").into())
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
    let mut message = format!("{source} → {target}");
    if !exposed_secrets.is_empty() {
        message.push_str(&format!(" (exposed: {})", exposed_secrets.join(", ")));
    }
    let mut ev = Event::new(
        "bind",
        class::FILE_ACTIVITY,
        category::SYSTEM,
        activity::FILE_MOUNT,
        sev,
        ctx,
    )
    .set("message", message.into())
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

pub fn sandbox_run(ctx: &Context, reference: &str, digest: &str, policy_hash: &str) -> Value {
    let ev = Event::new(
        "sandbox_run",
        class::PROCESS_ACTIVITY,
        category::SYSTEM,
        activity::PROCESS_LAUNCH,
        severity::INFORMATIONAL,
        ctx,
    )
    .set("message", format!("ran sandbox {reference}").into())
    .set("process", json!({"uid": ctx.run, "name": "sandbox"}))
    .set("device", microvm_device(ctx))
    .set("actor", lns_actor())
    .note("lns_origin", "host".into())
    .note("lns_sandbox", reference.into())
    .note("lns_sandbox_digest", digest.into())
    .note("lns_policy_hash", policy_hash.into());
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
    fn a_grant_names_the_connector_the_method_and_the_account_behind_it() {
        let ev = connector(
            &ctx(),
            "some-provider",
            "granted",
            Some("token"),
            Some("work"),
            Some("sha256:abc"),
            Some("flag"),
        );
        assert_schema_valid(&ev);
        assert_eq!(ev["message"], "granted some-provider token as work");
        assert_eq!(ev["unmapped"]["lns_connector"], "some-provider");
        assert_eq!(ev["unmapped"]["lns_verb"], "granted");
        assert_eq!(ev["unmapped"]["lns_method"], "token");
        assert_eq!(ev["unmapped"]["lns_connection"], "work");
        assert_eq!(
            ev["unmapped"]["lns_connector_digest"], "sha256:abc",
            "a grant binds to bytes, and which bytes is the whole of what it consented to"
        );
        assert_eq!(ev["service"]["name"], "some-provider");
        assert_eq!(
            ev["unmapped"]["lns_answer_source"], "flag",
            "who answered is part of what the chain accounts for"
        );
    }

    #[test]
    fn a_grant_of_a_method_that_authenticates_nothing_names_no_account() {
        let ev = connector(
            &ctx(),
            "some-provider",
            "granted",
            Some("open"),
            None,
            Some("sha256:abc"),
            Some("card"),
        );
        assert_schema_valid(&ev);
        assert_eq!(ev["message"], "granted some-provider open");
        assert!(ev["unmapped"].get("lns_connection").is_none());
        assert_eq!(ev["unmapped"]["lns_answer_source"], "card");
    }

    #[test]
    fn a_decline_and_a_forget_name_the_connector_alone() {
        for (verb, expected) in [
            ("declined", "declined some-provider"),
            ("forgot", "forgot some-provider"),
        ] {
            let ev = connector(&ctx(), "some-provider", verb, None, None, None, None);
            assert_schema_valid(&ev);
            assert_eq!(ev["message"], expected);
            assert!(
                ev["unmapped"].get("lns_connector_digest").is_none(),
                "neither answers for one version of the bytes, so neither claims one"
            );
            assert!(
                ev["unmapped"].get("lns_answer_source").is_none(),
                "a line with no answer source reads as it always did"
            );
        }
    }

    #[test]
    fn tool_provision_discloses_what_was_fetched_from_where_and_the_resolution() {
        let ev = tool_provision(
            &ctx(),
            "node",
            "node@22",
            "22.11.0",
            Some("nodejs.org"),
            "core:node",
        );
        assert_schema_valid(&ev);
        assert_eq!(
            ev["message"],
            "provisioned node@22 → 22.11.0 from nodejs.org"
        );
        assert_eq!(ev["file"]["name"], "/.lens/tools/node/22.11.0");
        assert_eq!(ev["unmapped"]["lns_origin"], "host");
        assert_eq!(ev["unmapped"]["lns_tool"], "node");
        assert_eq!(ev["unmapped"]["lns_requested"], "node@22");
        assert_eq!(ev["unmapped"]["lns_resolved"], "22.11.0");
        assert_eq!(ev["unmapped"]["lns_source"], "nodejs.org");
        assert_eq!(ev["unmapped"]["lns_backend"], "core:node");
        assert_eq!(
            (&ev["activity_name"], &ev["type_uid"]),
            (&json!("Create"), &json!(100101)),
            "acquisition creates a cache tree; nothing is mounted at record time"
        );
        assert_eq!(ev["device"]["name"], "calm-finch");
    }

    #[test]
    fn a_backend_that_names_no_host_attests_none() {
        // The shipped snapshot carries `core:elixir` and `http:dart`, which say nothing about where the bytes come from.
        let ev = tool_provision(
            &ctx(),
            "elixir",
            "elixir@1.18",
            "1.18.1",
            None,
            "core:elixir",
        );
        assert_schema_valid(&ev);
        assert_eq!(ev["message"], "provisioned elixir@1.18 → 1.18.1");
        assert!(
            ev["unmapped"].get("lns_source").is_none(),
            "a guessed host would be a false attestation: {ev}"
        );
        assert_eq!(ev["unmapped"]["lns_backend"], "core:elixir");
    }

    #[test]
    fn a_tool_provisioned_by_a_pull_names_no_device() {
        // `lns pull` provisions before any microVM exists, so claiming a virtual device named "" would be a lie to a SIEM.
        let pull = Context {
            time_unix_secs: 1_780_000_000,
            ts_rfc3339: "2026-06-29T14:00:00Z",
            run: "pull-1a2b3c4d5e6f",
            microvm: "",
        };
        let ev = tool_provision(
            &pull,
            "node",
            "node@22",
            "22.11.0",
            Some("nodejs.org"),
            "core:node",
        );
        assert_schema_valid(&ev);
        assert!(ev.get("device").is_none(), "got: {ev}");
    }

    #[test]
    fn an_allowed_approval_is_an_informational_finding_with_allowed_disposition() {
        let ev = approval(
            &ctx(),
            "network",
            "api.example.test:443",
            "allow_always",
            Some("policy-ambiguous"),
        );
        assert_schema_valid(&ev);
        assert_eq!(ev["class_uid"], 2004);
        assert_eq!(ev["class_name"], "Detection Finding");
        assert_eq!(
            ev["message"],
            "network allow-always api.example.test:443  [needs your decision]"
        );
        assert_eq!(ev["severity_id"], 1);
        assert_eq!(ev["disposition_id"], 1);
        assert_eq!(ev["disposition"], "Allowed");
        assert_eq!(ev["finding_info"]["uid"], "api.example.test:443");
        assert_eq!(ev["finding_info"]["title"], "needs your decision");
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
            "network",
            "api.example.test:443",
            "deny_always",
            None,
        );
        assert_schema_valid(&ev);
        assert_eq!(ev["severity_id"], 3);
        assert_eq!(ev["disposition_id"], 2);
        assert_eq!(ev["disposition"], "Blocked");
        assert_eq!(ev["finding_info"]["title"], "network");
        assert_eq!(ev["unmapped"]["lns_decision"], "deny_always");
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
        assert_eq!(
            ev["message"],
            "GET api.example.test:443 — allowed once → 200 success"
        );
        assert_eq!(
            ev["dst_endpoint"],
            json!({"domain": "api.example.test", "port": 443})
        );
        assert_eq!(ev["status_detail"], "allowed once");
        assert_eq!(ev["activity_id"], 3);
        assert_eq!(ev["http_request"]["http_method"], "GET");
        assert_eq!(
            ev["http_request"]["url"]["text"],
            "http://api.example.test:443/"
        );
        assert_eq!(ev["http_response"]["code"], 200);
        assert_eq!(ev["status_id"], 1);
        assert_eq!(ev["status"], "Success");
        assert_eq!(ev["activity_name"], "Get");
        assert_eq!(ev["unmapped"]["lns_result"], "success");
        assert_eq!(ev["disposition_id"], 1);
        assert_eq!(ev["disposition"], "Allowed");
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
        assert_eq!(ev["activity_name"], "Post");
        assert_eq!(ev["status_id"], 2);
        assert_eq!(ev["status"], "Failure");
        assert_eq!(ev["unmapped"]["lns_result"], "error");
        assert_eq!(ev["disposition_id"], 2);
        assert_eq!(ev["disposition"], "Blocked");
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
    fn workload_launch_records_the_image_as_a_process_activity() {
        let ev = workload_launch(&ctx(), "alpine:latest");
        assert_schema_valid(&ev);
        assert_eq!(ev["class_uid"], 1007);
        assert_eq!(ev["message"], "launched alpine:latest");
        assert_eq!(ev["process"]["uid"], "9e8d7c6b0000");
        assert_eq!(ev["device"]["name"], "calm-finch");
        assert_eq!(ev["actor"]["app_name"], "lns");
        assert_eq!(ev["unmapped"]["lns_kind"], "launch");
        assert_eq!(ev["unmapped"]["lns_image"], "alpine:latest");
        assert_eq!(ev["unmapped"]["lns_origin"], "host");
    }

    #[test]
    fn workload_exit_records_the_code_and_how_the_run_ended() {
        let ev = workload_exit(&ctx(), 137, true);
        assert_schema_valid(&ev);
        assert_eq!(ev["class_uid"], 1007);
        assert_eq!(ev["activity_id"], 2);
        assert_eq!(ev["message"], "workload killed with code 137");
        assert_eq!(ev["unmapped"]["lns_exit_code"], 137);
        assert_eq!(ev["unmapped"]["lns_killed"], true);
        let graceful = workload_exit(&ctx(), 0, false);
        assert_eq!(graceful["message"], "workload exited with code 0");
    }

    #[test]
    fn no_dhcp_lease_records_a_failed_boot_instead_of_a_workload_exit() {
        let ev = no_dhcp_lease(&ctx(), 125);
        assert_schema_valid(&ev);
        assert_eq!(ev["class_uid"], 1007);
        assert_eq!(ev["activity_id"], 2);
        assert_eq!(ev["status_id"], 2);
        assert_eq!(ev["unmapped"]["lns_kind"], "no_dhcp_lease");
        assert_eq!(ev["unmapped"]["lns_exit_code"], 125);
    }

    #[test]
    fn network_setup_failed_keeps_the_underlying_error_in_the_message() {
        let ev = network_setup_failed(&ctx(), 1, "`ip link set eth0 up` exited with 1");
        assert_schema_valid(&ev);
        assert_eq!(ev["class_uid"], 1007);
        assert_eq!(ev["status_id"], 2);
        assert_eq!(ev["unmapped"]["lns_kind"], "network_setup_failed");
        assert_eq!(
            ev["message"],
            "the guest could not set up its network: `ip link set eth0 up` exited with 1"
        );
    }

    #[test]
    fn workload_restart_notes_that_policy_re_resolves_live() {
        let ev = workload_restart(&ctx(), "alpine:latest");
        assert_schema_valid(&ev);
        assert_eq!(ev["class_uid"], 1007);
        assert_eq!(ev["unmapped"]["lns_kind"], "restart");
        assert!(ev["message"].as_str().unwrap().contains("re-resolved live"));
    }

    #[test]
    fn run_removed_says_how_the_removal_was_asked_for() {
        let ev = run_removed(&ctx(), false, false);
        assert_schema_valid(&ev);
        assert_eq!(ev["class_uid"], 1001);
        assert_eq!(ev["activity_id"], 4);
        assert!(ev["message"].as_str().unwrap().contains("lns rm"));
        assert_eq!(
            run_removed(&ctx(), true, false)["unmapped"]["lns_forced"],
            true
        );
        assert_eq!(
            run_removed(&ctx(), false, true)["unmapped"]["lns_auto"],
            true
        );
    }

    #[test]
    fn runs_pruned_names_what_it_removed() {
        let ev = runs_pruned(&ctx(), &["aa01".into(), "bb02".into()]);
        assert_schema_valid(&ev);
        assert_eq!(ev["class_uid"], 1001);
        assert!(ev["message"].as_str().unwrap().contains("aa01, bb02"));
    }

    #[test]
    fn sandbox_run_records_the_reference_and_the_digest_that_actually_ran() {
        let ev = sandbox_run(
            &ctx(),
            "some-registry.example/some-agent:research",
            "sha256:beef",
            "sha256:po1icy",
        );
        assert_schema_valid(&ev);
        assert_eq!(ev["class_uid"], 1007);
        assert_eq!(ev["unmapped"]["lns_kind"], "sandbox_run");
        assert_eq!(ev["unmapped"]["lns_origin"], "host");
        assert_eq!(
            ev["unmapped"]["lns_sandbox"],
            "some-registry.example/some-agent:research"
        );
        assert_eq!(
            ev["unmapped"]["lns_sandbox_digest"], "sha256:beef",
            "the audit must pin which bytes actually ran, not just the mutable tag"
        );
        assert_eq!(ev["unmapped"]["lns_policy_hash"], "sha256:po1icy");
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
        assert_eq!(ev["message"], "injected: OPENAI_API_KEY, PATH");
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
        assert_eq!(ev["message"], "data → /data");
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
        assert_eq!(ev["message"], "/Users/me/proj → /work (exposed: .env)");
        assert_eq!(ev["unmapped"]["lns_source"], "/Users/me/proj");
        assert_eq!(ev["unmapped"]["lns_exposed_secrets"][0], ".env");
        assert_eq!(ev["unmapped"]["lns_dropped_secrets"][1], ".ssh");
    }

    #[test]
    fn a_bind_with_no_exposed_secret_stays_informational() {
        let ev = bind_mount(&ctx(), "/src", "/work", &[], &[]);
        assert_schema_valid(&ev);
        assert_eq!(ev["severity_id"], 1);
        assert_eq!(ev["message"], "/src → /work");
        assert!(ev["unmapped"].get("lns_exposed_secrets").is_none());
        assert!(ev["unmapped"].get("lns_dropped_secrets").is_none());
    }

    #[test]
    fn phrasing_helpers_cover_every_decision_and_outcome_branch() {
        assert_eq!(decision_word("allow_once"), "allow-once");
        assert_eq!(decision_word("deny_always"), "deny-always");
        assert_eq!(reason_phrase("user-allowed-once"), "allowed once");
        assert_eq!(reason_phrase("user-allowed-persisted"), "allowed always");
        assert_eq!(reason_phrase("user-denied-once"), "denied once");
        assert_eq!(reason_phrase("user-denied-persisted"), "denied always");
        assert_eq!(reason_phrase("policy-ambiguous"), "needs your decision");
        assert_eq!(reason_phrase("policy-deny"), "blocked by policy");
        assert_eq!(reason_phrase("prefetch"), "prefetch");
        assert_eq!(
            egress_outcome(Some(200), Some("success")),
            Some("→ 200 success".to_string())
        );
        assert_eq!(egress_outcome(Some(204), None), Some("→ 204".to_string()));
        assert_eq!(
            egress_outcome(None, Some("error")),
            Some("→ error".to_string())
        );
        assert_eq!(egress_outcome(None, None), None);
    }

    #[test]
    fn endpoint_and_summary_helpers_cover_every_branch() {
        assert_eq!(request_summary("GET", "http://h:80/x"), "GET h:80");
        assert_eq!(
            request_summary("", "https://api.example.test/v1"),
            "api.example.test"
        );
        assert_eq!(request_summary("CONNECT", "host:443"), "CONNECT host:443");
        let ep = dst_endpoint("http://api.example.test:443/");
        assert_eq!(ep["domain"], "api.example.test");
        assert_eq!(ep["port"], 443);
        let bare = dst_endpoint("internal.svc");
        assert_eq!(bare["domain"], "internal.svc");
        assert!(bare.get("port").is_none());
        assert_eq!(
            dst_endpoint("http://host:notaport/")["domain"],
            "host:notaport"
        );
    }
}
