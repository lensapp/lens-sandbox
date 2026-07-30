use std::collections::{BTreeSet, HashMap};

use lns_audit::TimelineRow;
use lns_policy::connectors::{AuthKind, Connector};
use lns_policy::credentials::{CredentialEntry, CredentialStateFile};
use zeroize::Zeroizing;

use super::{
    CredentialBinding, CredentialStatus, CredentialSummary, DashboardCredential, DashboardSandbox,
    PendingCredentialRequest, SandboxAccess,
};

const EXPIRING_SOON_SECONDS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RunCredentialAccess {
    pub sandbox_id: String,
    pub grants: Vec<RunCredentialGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RunCredentialGrant {
    pub connector_id: String,
    pub reason: String,
    pub revocable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingCredential {
    pub id: String,
    pub connector_id: String,
    pub action: String,
    pub sandbox_id: String,
    pub sandbox_name: String,
    pub project: String,
    pub host_value_available: bool,
    pub bound_value_available: bool,
    pub oauth: bool,
    pub token_fallback: bool,
    pub verification_uri: Option<String>,
    pub user_code: Option<String>,
}

pub(super) struct SnapshotInput<'a> {
    pub catalog: &'a [Connector],
    pub credential_state: &'a CredentialStateFile,
    pub sandboxes: Vec<DashboardSandbox>,
    pub run_access: Vec<RunCredentialAccess>,
    pub pending: Vec<PendingCredential>,
    pub host_values: HashMap<String, Zeroizing<String>>,
    pub rows: &'a [TimelineRow],
    pub now: u64,
}

pub(super) fn build_credentials(input: SnapshotInput<'_>) -> Vec<DashboardCredential> {
    let connector_ids = visible_connector_ids(&input);
    connector_ids
        .into_iter()
        .map(|connector_id| build_credential(&connector_id, &input))
        .collect()
}

fn visible_connector_ids(input: &SnapshotInput<'_>) -> BTreeSet<String> {
    input
        .credential_state
        .keys()
        .cloned()
        .chain(
            input
                .run_access
                .iter()
                .flat_map(|access| access.grants.iter().map(|grant| grant.connector_id.clone())),
        )
        .chain(
            input
                .pending
                .iter()
                .map(|pending| pending.connector_id.clone()),
        )
        .chain(input.rows.iter().filter_map(|row| row.connector.clone()))
        .collect()
}

fn build_credential(connector_id: &str, input: &SnapshotInput<'_>) -> DashboardCredential {
    let connector = input
        .catalog
        .iter()
        .find(|connector| connector.id == connector_id);
    let entry = input.credential_state.get(connector_id);
    let pending = input
        .pending
        .iter()
        .find(|pending| pending.connector_id == connector_id);
    let binding = credential_binding(connector, entry);
    let status = credential_status(
        entry,
        pending.is_some(),
        input.host_values.contains_key(connector_id),
        input.now,
    );
    let (environment_variable, destinations) = connector_disclosure(connector);
    let (value, account, scopes, expires_at) =
        entry_disclosure(entry, input.host_values.get(connector_id), input.now);
    DashboardCredential {
        summary: CredentialSummary {
            connector_id: connector_id.to_string(),
            display_name: connector
                .map(|connector| connector.display_name().to_string())
                .unwrap_or_else(|| connector_id.to_string()),
            binding,
            status,
            account,
            scopes,
            expires_at,
            environment_variable,
            destinations,
            sandboxes: sandbox_access(connector_id, input),
            recent_activity: recent_activity(connector_id, input.rows, input.now),
            pending: pending.map(pending_request),
        },
        value,
    }
}

fn credential_binding(
    connector: Option<&Connector>,
    entry: Option<&CredentialEntry>,
) -> CredentialBinding {
    match entry {
        Some(CredentialEntry::Oauth { .. }) => CredentialBinding::OAuth,
        Some(CredentialEntry::Stored { .. })
            if connector.is_some_and(|connector| connector.auth_kind == AuthKind::Oauth) =>
        {
            CredentialBinding::OAuth
        }
        Some(CredentialEntry::Stored { .. }) => CredentialBinding::Stored,
        Some(CredentialEntry::HostDetect) => CredentialBinding::HostDetected,
        Some(CredentialEntry::Deny) => CredentialBinding::Denied,
        None => CredentialBinding::Unbound,
    }
}

fn credential_status(
    entry: Option<&CredentialEntry>,
    pending: bool,
    host_value_available: bool,
    now: u64,
) -> CredentialStatus {
    if pending {
        return CredentialStatus::Pending;
    }
    match entry {
        Some(CredentialEntry::Stored { value }) if !value.is_empty() => CredentialStatus::Active,
        Some(CredentialEntry::Oauth {
            access_token,
            expires_at,
            ..
        }) if access_token.is_empty() => CredentialStatus::Unavailable,
        Some(CredentialEntry::Oauth { expires_at, .. }) if *expires_at <= now => {
            CredentialStatus::Expired
        }
        Some(CredentialEntry::Oauth { expires_at, .. })
            if expires_at.saturating_sub(now) <= EXPIRING_SOON_SECONDS =>
        {
            CredentialStatus::Expiring
        }
        Some(CredentialEntry::Oauth { .. }) => CredentialStatus::Active,
        Some(CredentialEntry::HostDetect) if host_value_available => CredentialStatus::Active,
        Some(CredentialEntry::HostDetect) => CredentialStatus::Unavailable,
        Some(CredentialEntry::Deny) => CredentialStatus::Denied,
        Some(CredentialEntry::Stored { .. }) | None => CredentialStatus::Unavailable,
    }
}

fn connector_disclosure(connector: Option<&Connector>) -> (Option<String>, Vec<String>) {
    let Some(connector) = connector else {
        return (None, Vec::new());
    };
    let (environment_variable, injections) = match connector.auth_kind {
        AuthKind::Credential => connector
            .credential
            .as_ref()
            .map(|auth| (&auth.env_var, auth.injections.as_slice())),
        AuthKind::Oauth => connector
            .oauth
            .as_ref()
            .map(|auth| (&auth.env_var, auth.injections.as_slice())),
    }
    .map_or((None, &[][..]), |(env, injections)| {
        (Some(env.clone()), injections)
    });
    let destinations = injections
        .iter()
        .map(|injection| injection.domain.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    (environment_variable, destinations)
}

/// The usable value alongside what identifies it: the account, scopes, and expiry a user needs to recognize the binding. A host-detect entry's value is whatever the host resolves right now, so it comes from `host_value` rather than the file.
fn entry_disclosure(
    entry: Option<&CredentialEntry>,
    host_value: Option<&Zeroizing<String>>,
    now: u64,
) -> (
    Option<Zeroizing<String>>,
    Option<String>,
    Vec<String>,
    Option<String>,
) {
    match entry {
        Some(CredentialEntry::Stored { value }) => (nonempty(value), None, Vec::new(), None),
        Some(CredentialEntry::Oauth {
            access_token,
            expires_at,
            scopes,
            account,
            ..
        }) => (
            nonempty(access_token),
            account.clone(),
            scopes.clone(),
            Some(expiry_label(*expires_at, now)),
        ),
        Some(CredentialEntry::HostDetect) => (host_value.cloned(), None, Vec::new(), None),
        Some(CredentialEntry::Deny) | None => (None, None, Vec::new(), None),
    }
}

fn nonempty(value: &str) -> Option<Zeroizing<String>> {
    (!value.is_empty()).then(|| Zeroizing::new(value.to_string()))
}

fn expiry_label(expires_at: u64, now: u64) -> String {
    if expires_at <= now {
        "Expired".to_string()
    } else {
        crate::time_fmt::rfc3339_from_unix(expires_at)
    }
}

fn sandbox_access(connector_id: &str, input: &SnapshotInput<'_>) -> Vec<SandboxAccess> {
    let sandboxes: HashMap<&str, &DashboardSandbox> = input
        .sandboxes
        .iter()
        .map(|sandbox| (sandbox.id.as_str(), sandbox))
        .collect();
    let mut access = Vec::new();
    for grant in &input.run_access {
        let Some(credential_grant) = grant
            .grants
            .iter()
            .find(|candidate| candidate.connector_id == connector_id)
        else {
            continue;
        };
        if let Some(sandbox) = sandboxes.get(grant.sandbox_id.as_str()) {
            access.push(access_row(
                sandbox,
                &credential_grant.reason,
                sandbox.status == "running",
                credential_grant.revocable,
            ));
        }
    }
    for row in input
        .rows
        .iter()
        .filter(|row| row.connector.as_deref() == Some(connector_id))
    {
        let Some(sandbox) = input
            .sandboxes
            .iter()
            .find(|sandbox| sandbox.run_ids.iter().any(|run| run == &row.run))
        else {
            continue;
        };
        if access
            .iter()
            .any(|existing: &SandboxAccess| existing.sandbox_id == sandbox.id)
        {
            continue;
        }
        let running = sandbox.status == "running";
        let reason = if running {
            "Used by this running sandbox"
        } else {
            "Used by a previous run"
        };
        access.push(access_row(sandbox, reason, running, false));
    }
    access
}

fn access_row(
    sandbox: &DashboardSandbox,
    reason: &str,
    active: bool,
    revocable: bool,
) -> SandboxAccess {
    SandboxAccess {
        sandbox_id: sandbox.id.clone(),
        sandbox_name: sandbox.name.clone(),
        project: sandbox.project.clone(),
        reason: reason.to_string(),
        active,
        revocable,
    }
}

fn recent_activity(connector_id: &str, rows: &[TimelineRow], now: u64) -> Option<String> {
    rows.iter()
        .filter(|row| row.connector.as_deref() == Some(connector_id))
        .max_by(|left, right| left.ts.cmp(&right.ts))
        .map(|row| super::format::friendly_time(now as i64, &row.ts))
}

fn pending_request(pending: &PendingCredential) -> PendingCredentialRequest {
    PendingCredentialRequest {
        id: pending.id.clone(),
        sandbox_id: pending.sandbox_id.clone(),
        sandbox_name: pending.sandbox_name.clone(),
        project: pending.project.clone(),
        action: pending.action.clone(),
        host_value_available: pending.host_value_available,
        bound_value_available: pending.bound_value_available,
        oauth: pending.oauth,
        token_fallback: pending.token_fallback,
        verification_uri: pending.verification_uri.clone(),
        user_code: pending.user_code.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::connectors::{CredentialAuth, OauthAuth, OauthFlow};
    use lns_policy::providers::{InjectionDef, InjectionKind};
    use serde_json::Value;

    const NOW: u64 = 1_800_000_000;

    fn injection(domain: &str) -> InjectionDef {
        InjectionDef {
            kind: InjectionKind::BearerHeader,
            domain: domain.to_string(),
            header: None,
        }
    }

    fn credential_connector() -> Connector {
        Connector {
            id: "some-provider".into(),
            name: Some("Some Provider".into()),
            auth_kind: AuthKind::Credential,
            routes: Vec::new(),
            credential: Some(CredentialAuth {
                env_var: "SOME_TOKEN".into(),
                placeholder: "some-LNSPLACEHOLDER".into(),
                injections: vec![
                    injection("api.some-provider.example"),
                    injection("api.some-provider.example"),
                ],
            }),
            oauth: None,
            token_fallback: None,
        }
    }

    fn oauth_connector() -> Connector {
        Connector {
            id: "some-oauth".into(),
            name: Some("Some OAuth".into()),
            auth_kind: AuthKind::Oauth,
            routes: Vec::new(),
            credential: None,
            oauth: Some(OauthAuth {
                flow: OauthFlow::Device,
                client_id: Some("client".into()),
                client_secret: None,
                scopes: vec!["catalog-scope".into()],
                device_authorization_endpoint: Some("https://example.test/device".into()),
                authorization_endpoint: None,
                token_endpoint: "https://example.test/token".into(),
                userinfo_endpoint: None,
                account_field: None,
                env_var: "SOME_OAUTH_TOKEN".into(),
                placeholder: "oauth-LNSPLACEHOLDER".into(),
                injections: vec![injection("api.some-oauth.example")],
            }),
            token_fallback: None,
        }
    }

    fn sandbox(id: &str, status: &str) -> DashboardSandbox {
        DashboardSandbox {
            id: id.into(),
            name: format!("sandbox-{id}"),
            project: format!("/projects/{id}"),
            image: "example:latest".into(),
            status: status.into(),
            run_ids: vec![id.into()],
        }
    }

    fn row(run: &str, connector: &str, ts: &str) -> TimelineRow {
        TimelineRow {
            ts: ts.into(),
            when: ts.into(),
            run: run.into(),
            kind: "credential".into(),
            detail: "used credential".into(),
            raw: Value::Null,
            connector: Some(connector.into()),
        }
    }

    fn input<'a>(
        catalog: &'a [Connector],
        state: &'a CredentialStateFile,
        rows: &'a [TimelineRow],
    ) -> SnapshotInput<'a> {
        SnapshotInput {
            catalog,
            credential_state: state,
            sandboxes: vec![
                sandbox("running", "running"),
                sandbox("used", "running"),
                sandbox("past", ""),
            ],
            run_access: vec![RunCredentialAccess {
                sandbox_id: "running".into(),
                grants: vec![RunCredentialGrant {
                    connector_id: "some-provider".into(),
                    reason: "Connected by project policy".into(),
                    revocable: true,
                }],
            }],
            pending: Vec::new(),
            host_values: HashMap::new(),
            rows,
            now: NOW,
        }
    }

    #[test]
    fn stored_credential_combines_catalog_machine_policy_and_latest_activity() {
        let catalog = vec![credential_connector()];
        let state = HashMap::from([(
            "some-provider".into(),
            CredentialEntry::Stored {
                value: "some-secret".into(),
            },
        )]);
        let rows = vec![
            row(
                "past",
                "some-provider",
                &crate::time_fmt::rfc3339_from_unix(NOW - 120),
            ),
            row(
                "running",
                "some-provider",
                &crate::time_fmt::rfc3339_from_unix(NOW - 60),
            ),
            row(
                "used",
                "some-provider",
                &crate::time_fmt::rfc3339_from_unix(NOW - 90),
            ),
            row(
                "missing",
                "some-provider",
                &crate::time_fmt::rfc3339_from_unix(NOW - 180),
            ),
        ];
        let credentials = build_credentials(input(&catalog, &state, &rows));
        let credential = &credentials[0];
        assert_eq!(credential.summary.display_name, "Some Provider");
        assert_eq!(credential.summary.binding, CredentialBinding::Stored);
        assert_eq!(credential.summary.status, CredentialStatus::Active);
        assert_eq!(
            credential.summary.environment_variable.as_deref(),
            Some("SOME_TOKEN")
        );
        assert_eq!(
            credential.summary.destinations,
            ["api.some-provider.example"]
        );
        assert_eq!(
            credential.summary.recent_activity.as_deref(),
            Some("1m ago")
        );
        assert_eq!(
            credential.value.as_deref().map(String::as_str),
            Some("some-secret")
        );
        assert_eq!(credential.summary.sandboxes.len(), 3);
        let access: HashMap<_, _> = credential
            .summary
            .sandboxes
            .iter()
            .map(|access| (access.sandbox_id.as_str(), access.active))
            .collect();
        assert_eq!(
            access,
            HashMap::from([("running", true), ("used", true), ("past", false)])
        );
    }

    #[test]
    fn oauth_status_covers_expired_expiring_active_and_empty_tokens() {
        let connector = oauth_connector();
        for (access_token, expires_at, expected) in [
            ("token", NOW, CredentialStatus::Expired),
            (
                "token",
                NOW + EXPIRING_SOON_SECONDS,
                CredentialStatus::Expiring,
            ),
            (
                "token",
                NOW + EXPIRING_SOON_SECONDS + 1,
                CredentialStatus::Active,
            ),
            (
                "",
                NOW + EXPIRING_SOON_SECONDS + 1,
                CredentialStatus::Unavailable,
            ),
        ] {
            let catalog = vec![connector.clone()];
            let state = HashMap::from([(
                "some-oauth".into(),
                CredentialEntry::Oauth {
                    access_token: access_token.into(),
                    refresh_token: "refresh".into(),
                    expires_at,
                    scopes: vec!["granted-scope".into()],
                    account: Some("person@example.test".into()),
                },
            )]);
            let credentials = build_credentials(input(&catalog, &state, &[]));
            assert_eq!(credentials[0].summary.status, expected);
            assert_eq!(credentials[0].summary.binding, CredentialBinding::OAuth);
            assert_eq!(
                credentials[0].summary.account.as_deref(),
                Some("person@example.test")
            );
            assert_eq!(credentials[0].summary.scopes, ["granted-scope"]);
            assert!(credentials[0].summary.expires_at.is_some());
        }
    }

    #[test]
    fn pending_request_is_visible_without_pretending_to_know_its_sandbox() {
        let catalog = vec![credential_connector()];
        let state = CredentialStateFile::new();
        let mut source = input(&catalog, &state, &[]);
        source.pending = vec![PendingCredential {
            id: "request-1".into(),
            connector_id: "some-provider".into(),
            action: "connect to api.some-provider.example".into(),
            sandbox_id: "running".into(),
            sandbox_name: "calm-finch".into(),
            project: "~/project".into(),
            host_value_available: true,
            bound_value_available: true,
            oauth: false,
            token_fallback: false,
            verification_uri: None,
            user_code: None,
        }];
        let credentials = build_credentials(source);
        let credential = &credentials[0].summary;
        assert_eq!(credential.status, CredentialStatus::Pending);
        assert_eq!(credential.binding, CredentialBinding::Unbound);
        let pending = credential.pending.as_ref().expect("pending request");
        assert_eq!(pending.sandbox_id, "running");
        assert_eq!(pending.sandbox_name, "calm-finch");
        assert!(pending.host_value_available);
        assert!(pending.bound_value_available);
    }

    #[test]
    fn denied_host_detect_empty_stored_and_unknown_entries_remain_visible() {
        let catalog = vec![credential_connector()];
        let state = HashMap::from([
            ("denied".into(), CredentialEntry::Deny),
            ("host".into(), CredentialEntry::HostDetect),
            ("host-missing".into(), CredentialEntry::HostDetect),
            (
                "some-provider".into(),
                CredentialEntry::Stored {
                    value: String::new(),
                },
            ),
        ]);
        let mut source = input(&catalog, &state, &[]);
        source
            .host_values
            .insert("host".into(), Zeroizing::new("detected-secret".to_string()));
        let credentials = build_credentials(source);
        let by_id: HashMap<_, _> = credentials
            .iter()
            .map(|credential| {
                (
                    credential.summary.connector_id.as_str(),
                    (
                        credential.summary.binding,
                        credential.summary.status,
                        credential.value.as_deref().map(String::as_str),
                    ),
                )
            })
            .collect();
        assert_eq!(
            by_id["denied"],
            (CredentialBinding::Denied, CredentialStatus::Denied, None)
        );
        assert_eq!(
            by_id["host"],
            (
                CredentialBinding::HostDetected,
                CredentialStatus::Active,
                Some("detected-secret")
            )
        );
        assert_eq!(
            by_id["some-provider"],
            (
                CredentialBinding::Stored,
                CredentialStatus::Unavailable,
                None
            )
        );
        assert_eq!(
            by_id["host-missing"],
            (
                CredentialBinding::HostDetected,
                CredentialStatus::Unavailable,
                None
            )
        );
    }

    #[test]
    fn oauth_connector_with_a_durable_stored_key_reads_as_signed_in() {
        let catalog = vec![oauth_connector()];
        let state = HashMap::from([(
            "some-oauth".into(),
            CredentialEntry::Stored {
                value: "durable-key".into(),
            },
        )]);
        let credentials = build_credentials(input(&catalog, &state, &[]));
        assert_eq!(credentials[0].summary.binding, CredentialBinding::OAuth);
        assert_eq!(credentials[0].summary.status, CredentialStatus::Active);
        assert_eq!(
            credentials[0].value.as_deref().map(String::as_str),
            Some("durable-key")
        );
    }

    #[test]
    fn a_snapshot_never_debug_prints_the_values_it_carries() {
        let catalog = vec![credential_connector(), oauth_connector()];
        let state = HashMap::from([
            (
                "some-provider".into(),
                CredentialEntry::Stored {
                    value: "some-secret".into(),
                },
            ),
            (
                "some-oauth".into(),
                CredentialEntry::Oauth {
                    access_token: "some-access".into(),
                    refresh_token: "some-refresh".into(),
                    expires_at: NOW + 1,
                    scopes: vec!["granted-scope".into()],
                    account: Some("person@example.test".into()),
                },
            ),
            ("host".into(), CredentialEntry::HostDetect),
        ]);
        let mut source = input(&catalog, &state, &[]);
        source
            .host_values
            .insert("host".into(), Zeroizing::new("detected-secret".to_string()));

        let credentials = build_credentials(source);
        let rendered = format!("{credentials:?}");

        assert!(credentials.iter().any(
            |credential| credential.value.as_deref().map(String::as_str) == Some("some-secret")
        ));
        for secret in [
            "some-secret",
            "some-access",
            "some-refresh",
            "detected-secret",
        ] {
            assert!(
                !rendered.contains(secret),
                "{secret} reached the trace stream"
            );
        }
        assert!(rendered.contains("<redacted>"));
        assert!(rendered.contains("person@example.test"));
    }

    #[test]
    fn policy_and_audit_ids_are_visible_without_a_machine_entry() {
        let catalog = vec![credential_connector()];
        let state = CredentialStateFile::new();
        let rows = vec![row(
            "past",
            "audit-only",
            &crate::time_fmt::rfc3339_from_unix(NOW),
        )];
        let mut source = input(&catalog, &state, &rows);
        source.run_access.push(RunCredentialAccess {
            sandbox_id: "running".into(),
            grants: vec![RunCredentialGrant {
                connector_id: "policy-only".into(),
                reason: "Connected by project policy".into(),
                revocable: true,
            }],
        });
        let credentials = build_credentials(source);
        let ids: Vec<_> = credentials
            .iter()
            .map(|credential| credential.summary.connector_id.as_str())
            .collect();
        assert_eq!(ids, ["audit-only", "policy-only", "some-provider"]);
        assert_eq!(credentials[0].summary.display_name, "audit-only");
        assert_eq!(credentials[0].summary.status, CredentialStatus::Unavailable);
    }

    #[test]
    fn definition_grants_are_visible_and_not_revocable() {
        let catalog = vec![credential_connector()];
        let state = CredentialStateFile::new();
        let mut source = input(&catalog, &state, &[]);
        source.run_access[0].grants[0] = RunCredentialGrant {
            connector_id: "some-provider".into(),
            reason: "Required by sandbox definition".into(),
            revocable: false,
        };
        let credentials = build_credentials(source);
        let access = &credentials[0].summary.sandboxes[0];
        assert_eq!(access.reason, "Required by sandbox definition");
        assert!(!access.revocable);
    }
}
