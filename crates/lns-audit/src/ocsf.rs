use anyhow::{Context, Result, bail};
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
    let kind = req(um, "lns_kind")?.to_string();
    let detail = match kind.as_str() {
        "connection" => connection(um),
        "credential" => credential(um),
        "approval" => approval(um),
        "egress" => egress(event, um),
        "env" => env(um),
        "volume" => volume(um),
        "bind" => bind(um),
        other => bail!("OCSF audit event has unknown unmapped.lns_kind {other:?}"),
    };
    Ok(Row {
        kind,
        detail,
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

fn connection(um: &Map<String, Value>) -> String {
    format!(
        "connect {} ({}) {} [{}]",
        text(um, "lns_integration"),
        text(um, "lns_auth"),
        opt(um, "lns_account").unwrap_or_else(|| "-".to_string()),
        list(um, "lns_scopes").join(", ")
    )
}

fn credential(um: &Map<String, Value>) -> String {
    let key = opt(um, "lns_fp")
        .map(|fp| format!(" fp {fp}"))
        .unwrap_or_default();
    format!(
        "use {}{key} → {}",
        text(um, "lns_integration"),
        list(um, "lns_dest").join(", ")
    )
}

fn approval(um: &Map<String, Value>) -> String {
    let base = format!(
        "{} {} {}",
        text(um, "lns_approval_kind"),
        decision_word(&text(um, "lns_decision")),
        text(um, "lns_target")
    );
    match opt(um, "lns_reason") {
        Some(reason) => format!("{base}  [{reason}]"),
        None => base,
    }
}

fn egress(event: &Map<String, Value>, um: &Map<String, Value>) -> String {
    let mut detail = request_summary(&action(event));
    if let Some(reason) = opt(um, "lns_reason") {
        detail.push_str(&format!(" — {}", decision_phrase(&reason)));
    }
    if let Some(outcome) = outcome(event, um) {
        detail.push(' ');
        detail.push_str(&outcome);
    }
    detail
}

fn env(um: &Map<String, Value>) -> String {
    let keys = um
        .get("lns_env")
        .and_then(Value::as_object)
        .map(|env| env.keys().cloned().collect::<Vec<_>>().join(", "))
        .unwrap_or_default();
    format!("injected: {keys}")
}

fn volume(um: &Map<String, Value>) -> String {
    format!("{} → {}", named(um, "lns_name"), named(um, "lns_target"))
}

fn bind(um: &Map<String, Value>) -> String {
    let mut detail = format!("{} → {}", named(um, "lns_source"), named(um, "lns_target"));
    let exposed = list(um, "lns_exposed_secrets");
    if !exposed.is_empty() {
        detail.push_str(&format!(" (exposed: {})", exposed.join(", ")));
    }
    detail
}

fn decision_word(decision: &str) -> String {
    decision.replace('_', "-")
}

fn action(event: &Map<String, Value>) -> String {
    let request = event.get("http_request");
    let method = request
        .and_then(|r| r.get("http_method"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let url = request
        .and_then(|r| r.get("url"))
        .and_then(|u| u.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if method.is_empty() {
        url.to_string()
    } else {
        format!("{method} {url}")
    }
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

fn outcome(event: &Map<String, Value>, um: &Map<String, Value>) -> Option<String> {
    let status = event
        .get("http_response")
        .and_then(|r| r.get("code"))
        .and_then(Value::as_u64);
    let result = opt(um, "lns_result");
    match (status, result) {
        (Some(status), Some(result)) => Some(format!("→ {status} {result}")),
        (Some(status), None) => Some(format!("→ {status}")),
        (None, Some(result)) => Some(format!("→ {result}")),
        (None, None) => None,
    }
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

fn named(um: &Map<String, Value>, key: &str) -> String {
    um.get(key)
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string()
}

fn opt(um: &Map<String, Value>, key: &str) -> Option<String> {
    um.get(key).and_then(Value::as_str).map(str::to_string)
}

fn list(um: &Map<String, Value>, key: &str) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn octx<'a>(run: &'a str) -> lns_ocsf::Context<'a> {
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
    fn a_connection_reads_the_run_identity_and_renders_the_ledger_line() {
        let scopes = vec!["repo".to_string(), "read:org".to_string()];
        let ev = lns_ocsf::connection(
            &octx("9e8d7c6b0000"),
            "some-oauth",
            "oauth",
            Some("@user"),
            &scopes,
            None,
        );
        let row = read(&obj(&ev)).unwrap();
        assert_eq!(row.kind, "connection");
        assert_eq!(row.run, "9e8d7c6b0000");
        assert_eq!(row.ts, "2026-06-29T14:00:00Z");
        assert_eq!(row.integration.as_deref(), Some("some-oauth"));
        assert_eq!(
            row.detail,
            "connect some-oauth (oauth) @user [repo, read:org]"
        );
    }

    #[test]
    fn a_connection_without_an_account_shows_a_dash_and_empty_scopes() {
        let ev = lns_ocsf::connection(&octx("r"), "some-oauth", "apikey", None, &[], None);
        assert_eq!(
            read(&obj(&ev)).unwrap().detail,
            "connect some-oauth (apikey) - []"
        );
    }

    #[test]
    fn a_credential_use_renders_the_fingerprint_and_dests() {
        let ev = lns_ocsf::credential_use(
            &octx("r"),
            "some-provider",
            "apikey",
            Some("9c2f1a3d"),
            &["api.some-provider.example".to_string()],
        );
        let row = read(&obj(&ev)).unwrap();
        assert_eq!(row.kind, "credential");
        assert_eq!(
            row.detail,
            "use some-provider fp 9c2f1a3d → api.some-provider.example"
        );
    }

    #[test]
    fn a_credential_use_without_a_fingerprint_omits_the_double_space() {
        let ev = lns_ocsf::credential_use(&octx("r"), "some-provider", "apikey", None, &[]);
        let detail = read(&obj(&ev)).unwrap().detail;
        assert_eq!(detail, "use some-provider → ");
        assert!(!detail.contains("  "), "no double space: {detail}");
    }

    #[test]
    fn every_approval_decision_renders_with_hyphens_and_the_reason() {
        for (word, rendered) in [
            ("allow_once", "allow-once"),
            ("allow_always", "allow-always"),
            ("deny_once", "deny-once"),
            ("deny_always", "deny-always"),
            ("allow", "allow"),
            ("deny", "deny"),
        ] {
            let ev = lns_ocsf::approval(
                &octx("r"),
                "network",
                "api.foo.com:443",
                word,
                Some("policy-ambiguous"),
                None,
            );
            assert_eq!(
                read(&obj(&ev)).unwrap().detail,
                format!("network {rendered} api.foo.com:443  [policy-ambiguous]")
            );
        }
    }

    #[test]
    fn an_approval_without_a_reason_omits_the_bracket_and_carries_the_integration() {
        let ev = lns_ocsf::approval(
            &octx("r"),
            "credential",
            "some-provider",
            "deny_once",
            None,
            Some("some-provider"),
        );
        let row = read(&obj(&ev)).unwrap();
        assert_eq!(row.detail, "credential deny-once some-provider");
        assert_eq!(row.integration.as_deref(), Some("some-provider"));
    }

    #[test]
    fn an_egress_reads_like_the_approval_prompt_with_the_outcome() {
        let ev = lns_ocsf::egress(
            &octx("r"),
            "GET",
            "http://www.google.com:80/",
            Some(200),
            Some("success"),
            Some("user-allowed-once"),
            true,
        );
        let row = read(&obj(&ev)).unwrap();
        assert_eq!(row.kind, "egress");
        assert_eq!(
            row.detail,
            "GET www.google.com:80 — allowed once → 200 success"
        );
        assert!(row.integration.is_none());
    }

    #[test]
    fn an_egress_without_a_method_is_just_the_host() {
        let ev = lns_ocsf::egress(
            &octx("r"),
            "",
            "api.some-oauth.example:443",
            None,
            None,
            None,
            true,
        );
        assert_eq!(
            read(&obj(&ev)).unwrap().detail,
            "api.some-oauth.example:443"
        );
    }

    #[test]
    fn an_egress_shows_a_bare_status_when_there_is_no_result() {
        let mut ev = obj(&lns_ocsf::egress(
            &octx("r"),
            "GET",
            "http://x/",
            Some(204),
            None,
            None,
            true,
        ));
        assert!(
            ev["unmapped"].get("lns_result").is_none(),
            "no result was set"
        );
        assert_eq!(read(&ev).unwrap().detail, "GET x → 204");
        ev.remove("http_response");
        assert_eq!(read(&ev).unwrap().detail, "GET x", "no status, no result");
    }

    #[test]
    fn decision_phrase_maps_every_prompt_reason_and_passes_others_through() {
        assert_eq!(decision_phrase("user-allowed-once"), "allowed once");
        assert_eq!(decision_phrase("user-allowed-persisted"), "allowed always");
        assert_eq!(decision_phrase("user-denied-once"), "denied once");
        assert_eq!(decision_phrase("user-denied-persisted"), "denied always");
        assert_eq!(decision_phrase("policy-ambiguous"), "policy-ambiguous");
    }

    #[test]
    fn an_egress_shows_a_result_even_without_a_status_code() {
        let ev = lns_ocsf::egress(
            &octx("r"),
            "GET",
            "http://x/",
            None,
            Some("failure"),
            None,
            true,
        );
        assert_eq!(read(&obj(&ev)).unwrap().detail, "GET x → failure");
    }

    #[test]
    fn an_env_event_lists_the_injected_keys() {
        let mut env_map = Map::new();
        env_map.insert("OPENAI_API_KEY".into(), "…".into());
        env_map.insert("PATH".into(), "…".into());
        let ev = lns_ocsf::run_env(&octx("r"), &env_map);
        assert_eq!(
            read(&obj(&ev)).unwrap().detail,
            "injected: OPENAI_API_KEY, PATH"
        );
    }

    #[test]
    fn a_volume_and_a_bind_render_their_paths() {
        let volume = lns_ocsf::volume_mount(&octx("r"), "prism-data", "/data");
        assert_eq!(read(&obj(&volume)).unwrap().detail, "prism-data → /data");

        let bind = lns_ocsf::bind_mount(
            &octx("r"),
            "/Users/me/proj",
            "/work",
            &[".env".to_string()],
            &[".npmrc".to_string()],
        );
        assert_eq!(
            read(&obj(&bind)).unwrap().detail,
            "/Users/me/proj → /work (exposed: .env)"
        );
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
    fn an_event_without_an_unmapped_object_is_rejected() {
        let err = read(&obj(&serde_json::json!({"class_uid": 3002}))).unwrap_err();
        assert!(format!("{err:#}").contains("no unmapped object"), "{err:#}");
    }

    #[test]
    fn an_event_missing_its_kind_is_rejected() {
        let err = read(&obj(&serde_json::json!({"unmapped": {"lns_run": "r"}}))).unwrap_err();
        assert!(format!("{err:#}").contains("lns_kind"), "{err:#}");
    }

    #[test]
    fn an_unknown_kind_is_rejected() {
        let err = read(&obj(
            &serde_json::json!({"unmapped": {"lns_kind": "teleport"}}),
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("teleport"), "{err:#}");
    }

    #[test]
    fn volume_fields_fall_back_to_a_question_mark_when_absent() {
        let ev = serde_json::json!({"unmapped": {"lns_kind": "volume", "lns_name": "data"}});
        assert_eq!(read(&obj(&ev)).unwrap().detail, "data → ?");
    }
}
