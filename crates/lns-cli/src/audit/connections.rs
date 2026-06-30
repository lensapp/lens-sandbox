use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use anyhow::Result;
use lns_ipc::{AuthKind, LedgerEvent, LedgerRecord};

use super::store;
use super::table::render_table;
use super::{ConnectionsArgs, auth_word, friendly_when};

pub fn run(args: &ConnectionsArgs, out: &mut dyn Write) -> Result<i32> {
    let path = lns_ipc::connection_ledger()?;
    let records = store::read_ledger(&path)?;
    run_with_records(args, &records, out)
}

#[derive(Default)]
struct Conn {
    auth: Option<AuthKind>,
    account: Option<String>,
    scopes: Vec<String>,
    expires: Option<String>,
    dests: BTreeSet<String>,
    fingerprints: BTreeSet<String>,
    runs: BTreeSet<u32>,
    last_used: String,
    key_uses: BTreeMap<String, KeyUse>,
}

#[derive(Default)]
struct KeyUse {
    runs: BTreeSet<u32>,
    first: String,
    last: String,
}

fn run_with_records(
    args: &ConnectionsArgs,
    records: &[LedgerRecord],
    out: &mut dyn Write,
) -> Result<i32> {
    let connections = aggregate(records);
    match &args.integration {
        Some(integration) => render_detail(integration, connections.get(integration), out)?,
        None => render_summary(&connections, out)?,
    }
    Ok(0)
}

fn aggregate(records: &[LedgerRecord]) -> BTreeMap<String, Conn> {
    let mut map: BTreeMap<String, Conn> = BTreeMap::new();
    for record in records {
        let Some(integration) = record.event.integration() else {
            continue;
        };
        let conn = map.entry(integration.to_string()).or_default();
        conn.runs.insert(record.run);
        if record.ts > conn.last_used {
            conn.last_used = record.ts.clone();
        }
        absorb(conn, record);
    }
    map
}

fn absorb(conn: &mut Conn, record: &LedgerRecord) {
    match &record.event {
        LedgerEvent::Connection {
            auth,
            account,
            scopes,
            expires,
            ..
        } => {
            conn.auth = Some(*auth);
            if account.is_some() {
                conn.account = account.clone();
            }
            conn.scopes = scopes.clone();
            conn.expires = expires.clone();
        }
        LedgerEvent::CredentialUse { auth, fp, dest, .. } => {
            conn.auth.get_or_insert(*auth);
            conn.dests.extend(dest.iter().cloned());
            if let Some(fp) = fp {
                conn.fingerprints.insert(fp.clone());
                record_key_use(conn.key_uses.entry(fp.clone()).or_default(), record);
            }
        }
        LedgerEvent::Approval { .. } => {}
    }
}

fn record_key_use(use_: &mut KeyUse, record: &LedgerRecord) {
    use_.runs.insert(record.run);
    if use_.first.is_empty() || record.ts < use_.first {
        use_.first = record.ts.clone();
    }
    if record.ts > use_.last {
        use_.last = record.ts.clone();
    }
}

fn render_summary(connections: &BTreeMap<String, Conn>, out: &mut dyn Write) -> Result<()> {
    if connections.is_empty() {
        writeln!(out, "No connections recorded.")?;
        return Ok(());
    }
    let rows: Vec<Vec<String>> = connections
        .iter()
        .map(|(name, conn)| {
            vec![
                name.clone(),
                conn.auth.map(auth_word).unwrap_or("-").to_string(),
                identity(conn),
                scope_summary(conn),
                format_runs(&conn.runs),
                friendly_when(&conn.last_used),
            ]
        })
        .collect();
    render_table(
        out,
        &[
            "INTEGRATION",
            "AUTH",
            "IDENTITY",
            "SCOPE",
            "RUNS",
            "LAST USED",
        ],
        &rows,
    )?;
    Ok(())
}

fn render_detail(integration: &str, conn: Option<&Conn>, out: &mut dyn Write) -> Result<()> {
    let Some(conn) = conn else {
        writeln!(out, "No connections recorded for {integration}.")?;
        return Ok(());
    };
    writeln!(
        out,
        "{integration}  ({})",
        conn.auth.map(auth_word).unwrap_or("-")
    )?;
    if let Some(account) = &conn.account {
        writeln!(out, "  account    {account}")?;
    }
    if !conn.scopes.is_empty() {
        writeln!(out, "  scopes     {}", conn.scopes.join(", "))?;
    }
    if let Some(expires) = &conn.expires {
        writeln!(out, "  expires    {}", friendly_when(expires))?;
    }
    render_keys(conn, out)?;
    if !conn.dests.is_empty() {
        let dests: Vec<&String> = conn.dests.iter().collect();
        writeln!(
            out,
            "  injected   {}",
            dests
                .iter()
                .map(|d| d.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )?;
    }
    writeln!(out, "  runs       {}", format_runs(&conn.runs))?;
    writeln!(out, "  last used  {}", friendly_when(&conn.last_used))?;
    Ok(())
}

fn render_keys(conn: &Conn, out: &mut dyn Write) -> Result<()> {
    if conn.key_uses.is_empty() {
        return Ok(());
    }
    writeln!(out, "  keys used")?;
    for (fp, use_) in &conn.key_uses {
        writeln!(
            out,
            "    fp {fp}   runs {}   first {}   last {}",
            format_runs(&use_.runs),
            friendly_when(&use_.first),
            friendly_when(&use_.last)
        )?;
    }
    Ok(())
}

fn identity(conn: &Conn) -> String {
    if let Some(account) = &conn.account {
        return account.clone();
    }
    let mut fingerprints = conn.fingerprints.iter();
    match (fingerprints.next(), fingerprints.next()) {
        (None, _) => "-".to_string(),
        (Some(fp), None) => format!("fp {fp}"),
        (Some(_), Some(_)) => format!("{} keys", conn.fingerprints.len()),
    }
}

fn scope_summary(conn: &Conn) -> String {
    if !conn.scopes.is_empty() {
        return conn.scopes.join(", ");
    }
    if conn.dests.is_empty() {
        return "-".to_string();
    }
    conn.dests
        .iter()
        .map(|d| d.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_runs(runs: &BTreeSet<u32>) -> String {
    runs.iter()
        .map(|r| format!("#{r}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(run: u32, ts: &str, event: LedgerEvent) -> LedgerRecord {
        LedgerRecord {
            ts: ts.into(),
            run,
            microvm: format!("vm-{run}"),
            event,
        }
    }

    fn oauth_conn(with_account: bool) -> LedgerEvent {
        LedgerEvent::Connection {
            integration: "some-oauth".into(),
            auth: AuthKind::Oauth,
            account: with_account.then(|| "@hchen".to_string()),
            scopes: vec!["repo".into(), "read:org".into()],
            expires: Some("2026-07-29T00:00:00Z".into()),
        }
    }

    fn apikey_use(fp: &str, run_dest: &str) -> LedgerEvent {
        LedgerEvent::CredentialUse {
            integration: "some-provider".into(),
            auth: AuthKind::Apikey,
            fp: Some(fp.into()),
            dest: vec![run_dest.into()],
        }
    }

    fn summary_of(records: &[LedgerRecord]) -> String {
        let mut buf = Vec::new();
        run_with_records(&ConnectionsArgs { integration: None }, records, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    fn detail_of(integration: &str, records: &[LedgerRecord]) -> String {
        let mut buf = Vec::new();
        run_with_records(
            &ConnectionsArgs {
                integration: Some(integration.into()),
            },
            records,
            &mut buf,
        )
        .unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn an_empty_ledger_reports_no_connections() {
        assert_eq!(summary_of(&[]).trim(), "No connections recorded.");
    }

    #[test]
    fn the_summary_groups_oauth_and_apikey_integrations() {
        let records = vec![
            rec(41, "2026-06-28T10:14:00Z", oauth_conn(true)),
            rec(49, "2026-06-29T14:02:00Z", oauth_conn(true)),
            rec(
                49,
                "2026-06-29T14:05:00Z",
                apikey_use("9c2f1a3d", "api.some-provider.example"),
            ),
        ];
        let text = summary_of(&records);
        assert!(text.contains("some-oauth"));
        assert!(text.contains("oauth"));
        assert!(text.contains("@hchen"));
        assert!(text.contains("repo, read:org"));
        assert!(text.contains("#41, #49"));
        assert!(text.contains("some-provider"));
        assert!(text.contains("apikey"));
        assert!(text.contains("fp 9c2f1a3d"));
        assert!(text.contains("api.some-provider.example"));
    }

    #[test]
    fn multiple_keys_collapse_to_a_count_in_the_summary() {
        let records = vec![
            rec(
                41,
                "2026-06-20T00:00:00Z",
                apikey_use("9c2f1a3d", "api.some-provider.example"),
            ),
            rec(
                52,
                "2026-06-29T00:00:00Z",
                apikey_use("3e8b07aa", "api.some-provider.example"),
            ),
        ];
        let text = summary_of(&records);
        assert!(text.contains("2 keys"), "got: {text}");
    }

    #[test]
    fn the_oauth_drill_in_shows_account_scopes_and_expiry() {
        let records = vec![rec(49, "2026-06-29T14:02:00Z", oauth_conn(true))];
        let text = detail_of("some-oauth", &records);
        assert!(text.contains("some-oauth  (oauth)"));
        assert!(text.contains("account    @hchen"));
        assert!(text.contains("scopes     repo, read:org"));
        assert!(text.contains("expires    2026-07-29 00:00:00"));
        assert!(text.contains("runs       #49"));
    }

    #[test]
    fn the_apikey_drill_in_distinguishes_rotated_keys_per_run() {
        let records = vec![
            rec(
                41,
                "2026-06-20T00:00:00Z",
                apikey_use("9c2f1a3d", "api.some-provider.example"),
            ),
            rec(
                49,
                "2026-06-29T00:00:00Z",
                apikey_use("9c2f1a3d", "api.some-provider.example"),
            ),
            rec(
                52,
                "2026-06-29T12:00:00Z",
                apikey_use("3e8b07aa", "api.some-provider.example"),
            ),
        ];
        let text = detail_of("some-provider", &records);
        assert!(text.contains("keys used"));
        assert!(text.contains("fp 9c2f1a3d   runs #41, #49"));
        assert!(text.contains("fp 3e8b07aa   runs #52"));
        assert!(text.contains("injected   api.some-provider.example"));
    }

    #[test]
    fn drilling_into_an_unknown_integration_says_so() {
        let text = detail_of("nope", &[]);
        assert_eq!(text.trim(), "No connections recorded for nope.");
    }

    #[test]
    fn an_account_arriving_after_a_blank_connection_is_retained() {
        let records = vec![
            rec(40, "2026-06-19T00:00:00Z", oauth_conn(false)),
            rec(41, "2026-06-20T00:00:00Z", oauth_conn(true)),
        ];
        let text = detail_of("some-oauth", &records);
        assert!(text.contains("account    @hchen"));
    }

    #[test]
    fn an_approval_only_integration_appears_with_a_dash_identity() {
        let records = vec![rec(
            7,
            "2026-06-19T00:00:00Z",
            LedgerEvent::Approval {
                kind: lns_ipc::ApprovalKind::Integration,
                target: "some-oauth".into(),
                decision: lns_ipc::Decision::AllowOnce,
                reason: None,
                integration: Some("some-oauth".into()),
            },
        )];
        let summary = summary_of(&records);
        assert!(summary.contains("some-oauth"));
        let detail = detail_of("some-oauth", &records);
        assert!(detail.contains("some-oauth  (-)"));
        assert!(detail.contains("runs       #7"));
    }

    #[test]
    fn a_network_approval_without_an_integration_is_not_a_connection() {
        let records = vec![rec(
            9,
            "2026-06-29T00:00:00Z",
            LedgerEvent::Approval {
                kind: lns_ipc::ApprovalKind::Network,
                target: "api.foo.com:443".into(),
                decision: lns_ipc::Decision::AllowAlways,
                reason: None,
                integration: None,
            },
        )];
        assert_eq!(summary_of(&records).trim(), "No connections recorded.");
    }
}
