use std::io::Write;

use anyhow::{Context, Result};
use lns_ipc::{ApprovalKind, Decision, LedgerEvent, LedgerRecord};

use super::store;
use super::table::render_table;
use super::{LogArgs, auth_word, friendly_when};

pub fn run(args: &LogArgs, out: &mut dyn Write) -> Result<i32> {
    let path = lns_ipc::connection_ledger()?;
    let records = store::read_ledger(&path)?;
    run_with_records(args, &records, out)
}

fn run_with_records(args: &LogArgs, records: &[LedgerRecord], out: &mut dyn Write) -> Result<i32> {
    let selected: Vec<&LedgerRecord> = records.iter().filter(|r| matches_filter(r, args)).collect();
    if args.json {
        emit_json(&selected, out)?;
    } else {
        render(&selected, out)?;
    }
    Ok(0)
}

fn matches_filter(record: &LedgerRecord, args: &LogArgs) -> bool {
    if let Some(run) = args.run
        && record.run != run
    {
        return false;
    }
    if let Some(integration) = &args.integration
        && record.event.integration() != Some(integration.as_str())
    {
        return false;
    }
    if let Some(kind) = args.kind
        && record.event.name() != kind.event_name()
    {
        return false;
    }
    true
}

fn render(records: &[&LedgerRecord], out: &mut dyn Write) -> Result<()> {
    if records.is_empty() {
        writeln!(out, "No ledger events.")?;
        return Ok(());
    }
    let rows: Vec<Vec<String>> = records
        .iter()
        .map(|r| {
            vec![
                friendly_when(&r.ts),
                format!("#{}", r.run),
                r.event.name().to_string(),
                detail(&r.event),
            ]
        })
        .collect();
    render_table(out, &["WHEN", "RUN", "KIND", "DETAIL"], &rows)?;
    Ok(())
}

fn emit_json(records: &[&LedgerRecord], out: &mut dyn Write) -> Result<()> {
    for record in records {
        let line = serde_json::to_string(record).context("serializing ledger record")?;
        writeln!(out, "{line}")?;
    }
    Ok(())
}

fn detail(event: &LedgerEvent) -> String {
    match event {
        LedgerEvent::Approval {
            kind,
            target,
            decision,
            reason,
            ..
        } => {
            let base = format!(
                "{} {} {target}",
                approval_kind_word(*kind),
                decision_word(*decision)
            );
            match reason {
                Some(reason) => format!("{base}  [{reason}]"),
                None => base,
            }
        }
        LedgerEvent::Connection {
            integration,
            auth,
            account,
            scopes,
            ..
        } => {
            let who = account.as_deref().unwrap_or("-");
            format!(
                "connect {integration} ({}) {who} [{}]",
                auth_word(*auth),
                scopes.join(", ")
            )
        }
        LedgerEvent::CredentialUse {
            integration,
            fp,
            dest,
            ..
        } => {
            let key = fp.as_deref().map(|f| format!("fp {f}")).unwrap_or_default();
            format!("use {integration} {key} → {}", dest.join(", "))
        }
    }
}

fn decision_word(decision: Decision) -> &'static str {
    match decision {
        Decision::AllowOnce => "allow-once",
        Decision::AllowAlways => "allow-always",
        Decision::DenyOnce => "deny-once",
        Decision::DenyAlways => "deny-always",
        Decision::Allow => "allow",
        Decision::Deny => "deny",
    }
}

fn approval_kind_word(kind: ApprovalKind) -> &'static str {
    match kind {
        ApprovalKind::Network => "network",
        ApprovalKind::Credential => "credential",
        ApprovalKind::Integration => "integration",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::KindArg;
    use lns_ipc::AuthKind;

    fn rec(run: u32, event: LedgerEvent) -> LedgerRecord {
        LedgerRecord {
            ts: "2026-06-29T14:02:11Z".into(),
            run,
            microvm: "calm-finch".into(),
            event,
        }
    }

    fn approval() -> LedgerEvent {
        LedgerEvent::Approval {
            kind: ApprovalKind::Network,
            target: "api.foo.com:443".into(),
            decision: Decision::AllowAlways,
            reason: Some("policy-ambiguous".into()),
            integration: None,
        }
    }

    fn connection() -> LedgerEvent {
        LedgerEvent::Connection {
            integration: "github".into(),
            auth: AuthKind::Oauth,
            account: Some("@hchen".into()),
            scopes: vec!["repo".into(), "read:org".into()],
            expires: Some("2026-07-29T00:00:00Z".into()),
        }
    }

    fn credential_use() -> LedgerEvent {
        LedgerEvent::CredentialUse {
            integration: "open-router".into(),
            auth: AuthKind::Apikey,
            fp: Some("9c2f1a3d".into()),
            dest: vec!["api.openrouter.ai".into()],
        }
    }

    fn no_filter() -> LogArgs {
        LogArgs {
            integration: None,
            run: None,
            kind: None,
            json: false,
        }
    }

    fn render_to_string(args: &LogArgs, records: &[LedgerRecord]) -> String {
        let mut buf = Vec::new();
        run_with_records(args, records, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn an_empty_ledger_says_so() {
        let text = render_to_string(&no_filter(), &[]);
        assert_eq!(text.trim(), "No ledger events.");
    }

    #[test]
    fn the_timeline_renders_a_row_per_event_with_details() {
        let records = vec![
            rec(41, connection()),
            rec(49, credential_use()),
            rec(49, approval()),
        ];
        let text = render_to_string(&no_filter(), &records);
        assert!(text.contains("WHEN"));
        assert!(text.contains("2026-06-29 14:02:11"));
        assert!(text.contains("connect github (oauth) @hchen [repo, read:org]"));
        assert!(text.contains("use open-router fp 9c2f1a3d → api.openrouter.ai"));
        assert!(text.contains("network allow-always api.foo.com:443  [policy-ambiguous]"));
    }

    #[test]
    fn the_run_filter_keeps_only_matching_runs() {
        let records = vec![rec(41, connection()), rec(49, credential_use())];
        let args = LogArgs {
            run: Some(49),
            ..no_filter()
        };
        let text = render_to_string(&args, &records);
        assert!(text.contains("open-router"));
        assert!(!text.contains("github"));
    }

    #[test]
    fn the_integration_filter_keeps_only_matching_integrations() {
        let records = vec![rec(41, connection()), rec(49, credential_use())];
        let args = LogArgs {
            integration: Some("github".into()),
            ..no_filter()
        };
        let text = render_to_string(&args, &records);
        assert!(text.contains("github"));
        assert!(!text.contains("open-router"));
    }

    #[test]
    fn the_kind_filter_keeps_only_matching_event_kinds() {
        let records = vec![rec(49, approval()), rec(49, credential_use())];
        let args = LogArgs {
            kind: Some(KindArg::CredentialUse),
            ..no_filter()
        };
        let text = render_to_string(&args, &records);
        assert!(text.contains("open-router"));
        assert!(!text.contains("api.foo.com"));
    }

    #[test]
    fn json_output_emits_one_record_per_line() {
        let records = vec![rec(49, credential_use())];
        let args = LogArgs {
            json: true,
            ..no_filter()
        };
        let text = render_to_string(&args, &records);
        let parsed: LedgerRecord = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(parsed, records[0]);
    }

    #[test]
    fn an_approval_without_a_reason_omits_the_bracket() {
        let event = LedgerEvent::Approval {
            kind: ApprovalKind::Credential,
            target: "open-router".into(),
            decision: Decision::Deny,
            reason: None,
            integration: Some("open-router".into()),
        };
        assert_eq!(detail(&event), "credential deny open-router");
    }

    #[test]
    fn every_decision_and_auth_word_is_mapped() {
        for d in [
            Decision::AllowOnce,
            Decision::AllowAlways,
            Decision::DenyOnce,
            Decision::DenyAlways,
            Decision::Allow,
            Decision::Deny,
        ] {
            assert!(!decision_word(d).is_empty());
        }
        assert_eq!(auth_word(AuthKind::Oauth), "oauth");
        assert_eq!(auth_word(AuthKind::Apikey), "apikey");
        assert_eq!(approval_kind_word(ApprovalKind::Integration), "integration");
    }

    #[test]
    fn a_credential_use_without_a_fingerprint_still_renders() {
        let event = LedgerEvent::CredentialUse {
            integration: "some-provider".into(),
            auth: AuthKind::Apikey,
            fp: None,
            dest: vec![],
        };
        assert!(detail(&event).starts_with("use some-provider"));
    }
}
