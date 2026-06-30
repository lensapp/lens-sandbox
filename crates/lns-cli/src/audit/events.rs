use std::io::Write;

use anyhow::{Context, Result};
use lns_ipc::{ApprovalKind, Decision, LedgerEvent, LedgerRecord};

use super::store;
use super::table::render_table;
use super::{LogArgs, auth_word, friendly_when};

pub fn run(args: &LogArgs, out: &mut dyn Write) -> Result<i32> {
    let path = lns_ipc::connection_ledger()?;
    run_with_records(args, store::stream_ledger(&path)?, out)
}

fn run_with_records(
    args: &LogArgs,
    records: impl Iterator<Item = Result<LedgerRecord>>,
    out: &mut dyn Write,
) -> Result<i32> {
    if args.json {
        emit_json(args, records, out)?;
    } else {
        render(args, records, out)?;
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

fn render(
    args: &LogArgs,
    records: impl Iterator<Item = Result<LedgerRecord>>,
    out: &mut dyn Write,
) -> Result<()> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for record in records {
        let record = record?;
        if !matches_filter(&record, args) {
            continue;
        }
        rows.push(vec![
            friendly_when(&record.ts),
            format!("#{}", record.run),
            record.event.name().to_string(),
            detail(&record.event),
        ]);
    }
    if rows.is_empty() {
        writeln!(out, "No ledger events.")?;
        return Ok(());
    }
    render_table(out, &["WHEN", "RUN", "KIND", "DETAIL"], &rows)?;
    Ok(())
}

fn emit_json(
    args: &LogArgs,
    records: impl Iterator<Item = Result<LedgerRecord>>,
    out: &mut dyn Write,
) -> Result<()> {
    for record in records {
        let record = record?;
        if !matches_filter(&record, args) {
            continue;
        }
        let line = serde_json::to_string(&record).context("serializing ledger record")?;
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
            integration: "some-oauth".into(),
            auth: AuthKind::Oauth,
            account: Some("@hchen".into()),
            scopes: vec!["repo".into(), "read:org".into()],
            expires: Some("2026-07-29T00:00:00Z".into()),
        }
    }

    fn credential_use() -> LedgerEvent {
        LedgerEvent::CredentialUse {
            integration: "some-provider".into(),
            auth: AuthKind::Apikey,
            fp: Some("9c2f1a3d".into()),
            dest: vec!["api.some-provider.example".into()],
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
        run_with_records(args, records.iter().cloned().map(anyhow::Ok), &mut buf).unwrap();
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
        assert!(text.contains("connect some-oauth (oauth) @hchen [repo, read:org]"));
        assert!(text.contains("use some-provider fp 9c2f1a3d → api.some-provider.example"));
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
        assert!(text.contains("some-provider"));
        assert!(!text.contains("some-oauth"));
    }

    #[test]
    fn the_integration_filter_keeps_only_matching_integrations() {
        let records = vec![rec(41, connection()), rec(49, credential_use())];
        let args = LogArgs {
            integration: Some("some-oauth".into()),
            ..no_filter()
        };
        let text = render_to_string(&args, &records);
        assert!(text.contains("some-oauth"));
        assert!(!text.contains("some-provider"));
    }

    #[test]
    fn the_kind_filter_keeps_only_matching_event_kinds() {
        let records = vec![rec(49, approval()), rec(49, credential_use())];
        let args = LogArgs {
            kind: Some(KindArg::CredentialUse),
            ..no_filter()
        };
        let text = render_to_string(&args, &records);
        assert!(text.contains("some-provider"));
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
    fn json_output_skips_records_that_do_not_match_the_filter() {
        let records = vec![rec(41, connection()), rec(49, credential_use())];
        let args = LogArgs {
            run: Some(49),
            json: true,
            ..no_filter()
        };
        let text = render_to_string(&args, &records);
        assert_eq!(
            text.lines().filter(|l| !l.trim().is_empty()).count(),
            1,
            "only the run-49 record should be emitted as json"
        );
        assert!(text.contains("some-provider"));
        assert!(!text.contains("some-oauth"));
    }

    #[test]
    fn an_approval_without_a_reason_omits_the_bracket() {
        let event = LedgerEvent::Approval {
            kind: ApprovalKind::Credential,
            target: "some-provider".into(),
            decision: Decision::Deny,
            reason: None,
            integration: Some("some-provider".into()),
        };
        assert_eq!(detail(&event), "credential deny some-provider");
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
