use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use lns_policy::credentials::{
    CredentialEntry, CredentialStateFile, CredentialStore, JsonFileCredentialStore,
};
use lns_policy::grants::{GrantStore, JsonFileGrantStore, project_key};

use super::{CredentialReviewChoice, DashboardCommand};
use crate::approval_flow::window::WindowState;
use crate::credential_flow::session::CredentialDecisionRequest;

/// Where a command writes: the per-machine credential values and the per-workload grant sidecar, both outside anything a project commits.
pub(super) struct CommandStores {
    pub credentials: PathBuf,
    pub grants: PathBuf,
}

impl CommandStores {
    pub(super) fn default_paths() -> Self {
        Self {
            credentials: lns_policy::credentials::default_credentials_path(),
            grants: lns_policy::grants::default_workload_grants_path(),
        }
    }
}

pub(super) fn execute(
    command: &DashboardCommand,
    window_state: &WindowState,
    stores: &CommandStores,
) -> Result<String> {
    let notice = match command {
        DashboardCommand::ReviewCredential { request_id, choice } => {
            match request_id.strip_prefix(super::SIGN_IN_REQUEST_PREFIX) {
                Some(connector_id) => resolve_sign_in(window_state, connector_id, choice)?,
                None => decide_credential(window_state, request_id, choice)?,
            }
        }
        DashboardCommand::ReplaceCredential {
            connector_id,
            value,
        } => {
            update_credentials(&stores.credentials, |state| {
                replace_saved_credential(state, connector_id, value)
            })?;
            format!("{connector_id} was replaced.")
        }
        DashboardCommand::RemoveCredential { connector_id } => {
            update_credentials(&stores.credentials, |state| {
                remove_saved_credential(state, connector_id)
            })?;
            format!("{connector_id} was removed from this machine.")
        }
        DashboardCommand::DisconnectProject {
            connector_id,
            sandbox_id,
        } => {
            let policy_path = project_policy_path(sandbox_id, connector_id)?;
            disconnect_project(&policy_path, &stores.grants, connector_id)?;
            format!(
                "{connector_id} was disconnected from {}.",
                super::project_label(&policy_path)
            )
        }
    };
    crate::dashboard::live::note_write();
    Ok(notice)
}

/// A card whose sign-in is already running can only be cancelled or handed a pasted token; the value decisions belong to the card that raised it.
fn resolve_sign_in(
    window_state: &WindowState,
    connector_id: &str,
    choice: &CredentialReviewChoice,
) -> Result<String> {
    let (resolved, notice) = match choice {
        CredentialReviewChoice::Deny => (
            window_state.cancel_sign_in(connector_id),
            format!("Sign-in to {connector_id} was cancelled."),
        ),
        CredentialReviewChoice::UseValue(value) => (
            window_state.pivot_sign_in(connector_id, value.to_string()),
            format!("{connector_id} will use the supplied token."),
        ),
        CredentialReviewChoice::UseHost
        | CredentialReviewChoice::UseBound
        | CredentialReviewChoice::Connect => {
            bail!("sign-in for {connector_id} is already in progress")
        }
    };
    if !resolved {
        bail!("credential request is no longer pending");
    }
    Ok(notice)
}

fn decide_credential(
    window_state: &WindowState,
    request_id: &str,
    choice: &CredentialReviewChoice,
) -> Result<String> {
    // `AllowBound` spends the value already bound on this machine and leaves the stored entry alone; the others rebind it or sign in afresh.
    let request = match choice {
        CredentialReviewChoice::UseHost | CredentialReviewChoice::Connect => {
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect)
        }
        CredentialReviewChoice::UseBound => CredentialDecisionRequest::AllowBound,
        CredentialReviewChoice::UseValue(value) => {
            CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                value: value.to_string(),
            })
        }
        CredentialReviewChoice::Deny => CredentialDecisionRequest::Deny,
    };
    if !window_state.decide_credential(request_id, request) {
        bail!("credential request is no longer pending");
    }
    Ok(match choice {
        CredentialReviewChoice::Deny => "Credential request denied.".to_string(),
        CredentialReviewChoice::Connect => "Sign-in started.".to_string(),
        CredentialReviewChoice::UseHost
        | CredentialReviewChoice::UseBound
        | CredentialReviewChoice::UseValue(_) => "Credential request allowed.".to_string(),
    })
}

/// A connector the sandbox definition declares is part of how that workload launches, so dropping it from the project policy would not describe the sandbox in front of you.
fn project_policy_path(sandbox_id: &str, connector_id: &str) -> Result<PathBuf> {
    if crate::run_registry::credential_slots(sandbox_id)
        .iter()
        .any(|slot| slot.name == *connector_id)
    {
        bail!("{connector_id} is declared by the sandbox definition and cannot be revoked");
    }
    crate::run_registry::inspect(sandbox_id)
        .with_context(|| format!("sandbox {sandbox_id} is no longer available"))?
        .config
        .policy_path
        .map(PathBuf::from)
        .context("sandbox has no project policy")
}

/// Same two-step forget as `lns connector disconnect`: the grants go first, so a run still holding a card cannot record one against the id being dropped, and a later reconnect asks again instead of inheriting a stale grant.
fn disconnect_project(policy_path: &Path, grants_path: &Path, connector_id: &str) -> Result<()> {
    let mut policy = lns_policy::Policy::load_or_default(policy_path)
        .with_context(|| format!("reading policy {}", policy_path.display()))?;
    if !policy.disconnect(connector_id) {
        bail!("{connector_id} is no longer connected to this project");
    }
    let store = JsonFileGrantStore::new(grants_path.to_path_buf());
    GrantStore::update(&store, &mut |file| {
        file.revoke_project_connector(&project_key(policy_path), connector_id);
        true
    })
    .with_context(|| format!("updating grants at {}", grants_path.display()))?;
    policy
        .save_atomic(policy_path)
        .with_context(|| format!("saving policy {}", policy_path.display()))
}

fn update_credentials(
    path: &Path,
    mutate: impl FnOnce(&mut CredentialStateFile) -> Result<()>,
) -> Result<()> {
    let store = JsonFileCredentialStore::new(path.to_path_buf());
    let mut state = store
        .load()
        .with_context(|| format!("reading credential state {}", path.display()))?;
    mutate(&mut state)?;
    store
        .save(&state)
        .with_context(|| format!("saving credential state {}", path.display()))
}

fn replace_saved_credential(
    state: &mut CredentialStateFile,
    connector_id: &str,
    value: &str,
) -> Result<()> {
    match state.get_mut(connector_id) {
        Some(CredentialEntry::Stored { value: stored }) => stored.replace_range(.., value),
        Some(_) => bail!("{connector_id} is not a replaceable stored credential"),
        None => bail!("{connector_id} is no longer stored on this machine"),
    }
    Ok(())
}

fn remove_saved_credential(state: &mut CredentialStateFile, connector_id: &str) -> Result<()> {
    anyhow::ensure!(
        state.remove(connector_id).is_some(),
        "{connector_id} is no longer stored on this machine"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval_flow::window::{CredentialDecisionDelivery, SignInCard};
    use crate::credential_flow::session::{CredentialPendingPrompt, DenyScope};
    use lns_policy::grants::{GrantRecord, WorkloadGrantFile, WorkloadIdentity};
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    struct Fixture {
        dir: tempfile::TempDir,
        stores: CommandStores,
        window: std::sync::Arc<WindowState>,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            Self {
                stores: CommandStores {
                    credentials: dir.path().join("credentials.json"),
                    grants: dir.path().join("grants.json"),
                },
                window: WindowState::new(),
                dir,
            }
        }

        fn run(&self, command: &DashboardCommand) -> Result<String> {
            execute(command, &self.window, &self.stores)
        }

        fn seed_credentials(&self, entries: impl IntoIterator<Item = (String, CredentialEntry)>) {
            let store = JsonFileCredentialStore::new(self.stores.credentials.clone());
            store
                .save(&CredentialStateFile::from_iter(entries))
                .expect("seed credentials");
        }

        fn credentials(&self) -> CredentialStateFile {
            JsonFileCredentialStore::new(self.stores.credentials.clone())
                .load()
                .expect("load credentials")
        }

        fn pending(&self) -> mpsc::UnboundedReceiver<CredentialDecisionDelivery> {
            let (tx, rx) = mpsc::unbounded_channel();
            self.window.insert_credential_pending(
                CredentialPendingPrompt {
                    id: "request-1".into(),
                    credential_id: "some-provider".into(),
                    action: "use of some-provider placeholder".into(),
                    oauth_display_name: None,
                    token_fallback: None,
                    env_var: Some("SOME_TOKEN".into()),
                    injection_domains: vec!["api.some-provider.example".into()],
                    is_project_defined: false,
                    bound_value_available: true,
                    deny_scope: DenyScope::Workload,
                },
                true,
                tx,
            );
            rx
        }

        fn sign_in(&self) -> tokio::sync::oneshot::Receiver<crate::oauth::SignInPivot> {
            let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
            self.window.insert_sign_in(
                SignInCard {
                    credential_id: "some-oauth".into(),
                    display_name: "Some OAuth".into(),
                    user_code: Some("SOME-CODE".into()),
                    verification_uri: "https://api.some-oauth.example/device".into(),
                    token_fallback: None,
                    env_var: Some("SOME_OAUTH_TOKEN".into()),
                    injection_domains: vec!["api.some-oauth.example".into()],
                    is_project_defined: false,
                    origin: None,
                },
                cancel_tx,
            );
            cancel_rx
        }

        fn project_policy(&self, connectors: &[&str]) -> PathBuf {
            let path = self.dir.path().join("lns-policy.yaml");
            let mut policy = lns_policy::Policy::load_or_default(&path).expect("policy");
            for connector in connectors {
                policy.connect(*connector);
            }
            policy.save_atomic(&path).expect("save policy");
            path
        }

        fn seed_grant(&self, policy_path: &Path, connector: &str) -> String {
            let project = project_key(policy_path);
            let mut file = WorkloadGrantFile::default();
            file.upsert(GrantRecord::allow(
                &project,
                &WorkloadIdentity::Definition {
                    dir: project.clone(),
                },
                connector,
                "SOME_TOKEN",
                vec![],
            ));
            JsonFileGrantStore::new(self.stores.grants.clone())
                .save(&file)
                .expect("seed grants");
            project
        }

        fn grants(&self) -> WorkloadGrantFile {
            JsonFileGrantStore::new(self.stores.grants.clone())
                .load()
                .expect("load grants")
        }
    }

    fn review(choice: CredentialReviewChoice) -> DashboardCommand {
        DashboardCommand::ReviewCredential {
            request_id: "request-1".into(),
            choice,
        }
    }

    #[test]
    fn granting_the_bound_value_spends_it_without_rebinding_the_machine() {
        let fixture = Fixture::new();
        let mut rx = fixture.pending();

        let notice = fixture
            .run(&review(CredentialReviewChoice::UseBound))
            .expect("review succeeds");

        assert_eq!(notice, "Credential request allowed.");
        assert_eq!(
            rx.try_recv().expect("decision").request,
            CredentialDecisionRequest::AllowBound
        );
    }

    #[test]
    fn the_host_value_and_a_typed_value_each_bind_what_they_name() {
        for (choice, expected) in [
            (
                CredentialReviewChoice::UseHost,
                CredentialDecisionRequest::Allow(CredentialEntry::HostDetect),
            ),
            (
                CredentialReviewChoice::UseValue("some-secret".to_string().into()),
                CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                    value: "some-secret".into(),
                }),
            ),
        ] {
            let fixture = Fixture::new();
            let mut rx = fixture.pending();

            let notice = fixture.run(&review(choice)).expect("review succeeds");

            assert_eq!(notice, "Credential request allowed.");
            assert_eq!(rx.try_recv().expect("decision").request, expected);
        }
    }

    #[test]
    fn accepting_an_oauth_card_starts_a_sign_in_and_denying_one_records_the_refusal() {
        let fixture = Fixture::new();
        let mut rx = fixture.pending();

        assert_eq!(
            fixture
                .run(&review(CredentialReviewChoice::Connect))
                .expect("connect"),
            "Sign-in started."
        );
        assert_eq!(
            rx.try_recv().expect("decision").request,
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect)
        );

        let fixture = Fixture::new();
        let mut rx = fixture.pending();
        assert_eq!(
            fixture
                .run(&review(CredentialReviewChoice::Deny))
                .expect("deny"),
            "Credential request denied."
        );
        assert_eq!(
            rx.try_recv().expect("decision").request,
            CredentialDecisionRequest::Deny
        );
    }

    #[test]
    fn a_request_that_is_no_longer_pending_is_refused() {
        let fixture = Fixture::new();
        let error = fixture
            .run(&review(CredentialReviewChoice::Deny))
            .expect_err("stale request");
        assert!(error.to_string().contains("no longer pending"));
    }

    #[test]
    fn a_sign_in_in_progress_can_be_cancelled_or_handed_a_token() {
        let fixture = Fixture::new();
        let mut cancel_rx = fixture.sign_in();
        let notice = fixture
            .run(&DashboardCommand::ReviewCredential {
                request_id: "sign-in:some-oauth".into(),
                choice: CredentialReviewChoice::Deny,
            })
            .expect("cancel succeeds");
        assert_eq!(notice, "Sign-in to some-oauth was cancelled.");
        assert!(matches!(
            cancel_rx.try_recv(),
            Ok(crate::oauth::SignInPivot::Cancel)
        ));

        let fixture = Fixture::new();
        let mut cancel_rx = fixture.sign_in();
        let notice = fixture
            .run(&DashboardCommand::ReviewCredential {
                request_id: "sign-in:some-oauth".into(),
                choice: CredentialReviewChoice::UseValue("some-token".to_string().into()),
            })
            .expect("pivot succeeds");
        assert_eq!(notice, "some-oauth will use the supplied token.");
        assert!(matches!(
            cancel_rx.try_recv(),
            Ok(crate::oauth::SignInPivot::UseToken(token)) if token == "some-token"
        ));
    }

    #[test]
    fn a_sign_in_cannot_be_answered_with_a_value_decision() {
        for choice in [
            CredentialReviewChoice::UseBound,
            CredentialReviewChoice::UseHost,
            CredentialReviewChoice::Connect,
        ] {
            let fixture = Fixture::new();
            let _cancel_rx = fixture.sign_in();
            let error = fixture
                .run(&DashboardCommand::ReviewCredential {
                    request_id: "sign-in:some-oauth".into(),
                    choice,
                })
                .expect_err("sign-in already running");
            assert!(error.to_string().contains("already in progress"));
        }
    }

    #[test]
    fn a_sign_in_that_already_resolved_is_refused() {
        let fixture = Fixture::new();
        let error = fixture
            .run(&DashboardCommand::ReviewCredential {
                request_id: "sign-in:some-oauth".into(),
                choice: CredentialReviewChoice::Deny,
            })
            .expect_err("no card");
        assert!(error.to_string().contains("no longer pending"));
    }

    #[test]
    fn replacing_a_stored_value_keeps_every_other_decision() {
        let fixture = Fixture::new();
        fixture.seed_credentials([
            (
                "some-provider".to_string(),
                CredentialEntry::Stored {
                    value: "old-secret".into(),
                },
            ),
            ("some-host".to_string(), CredentialEntry::HostDetect),
        ]);

        let notice = fixture
            .run(&DashboardCommand::ReplaceCredential {
                connector_id: "some-provider".into(),
                value: "new-secret".to_string().into(),
            })
            .expect("replace succeeds");

        assert_eq!(notice, "some-provider was replaced.");
        let state = fixture.credentials();
        assert_eq!(
            state.get("some-provider"),
            Some(&CredentialEntry::Stored {
                value: "new-secret".into()
            })
        );
        assert_eq!(state.get("some-host"), Some(&CredentialEntry::HostDetect));
    }

    #[test]
    fn only_a_stored_value_can_be_replaced() {
        let fixture = Fixture::new();
        fixture.seed_credentials([("some-host".to_string(), CredentialEntry::HostDetect)]);

        let wrong_kind = fixture
            .run(&DashboardCommand::ReplaceCredential {
                connector_id: "some-host".into(),
                value: "new-secret".to_string().into(),
            })
            .expect_err("host-detect is not replaceable");
        assert!(wrong_kind.to_string().contains("not a replaceable"));

        let missing = fixture
            .run(&DashboardCommand::ReplaceCredential {
                connector_id: "some-provider".into(),
                value: "new-secret".to_string().into(),
            })
            .expect_err("nothing stored");
        assert!(missing.to_string().contains("no longer stored"));
        assert_eq!(
            fixture.credentials().get("some-host"),
            Some(&CredentialEntry::HostDetect)
        );
    }

    #[test]
    fn removing_a_credential_drops_only_that_decision() {
        let fixture = Fixture::new();
        fixture.seed_credentials([
            ("some-provider".to_string(), CredentialEntry::HostDetect),
            ("some-other".to_string(), CredentialEntry::Deny),
        ]);

        let notice = fixture
            .run(&DashboardCommand::RemoveCredential {
                connector_id: "some-provider".into(),
            })
            .expect("remove succeeds");

        assert_eq!(notice, "some-provider was removed from this machine.");
        let state = fixture.credentials();
        assert!(!state.contains_key("some-provider"));
        assert_eq!(state.get("some-other"), Some(&CredentialEntry::Deny));

        let error = fixture
            .run(&DashboardCommand::RemoveCredential {
                connector_id: "some-provider".into(),
            })
            .expect_err("already gone");
        assert!(error.to_string().contains("no longer stored"));
    }

    #[test]
    fn an_unreadable_credential_store_reports_its_path() {
        let fixture = Fixture::new();
        std::fs::create_dir(&fixture.stores.credentials).expect("directory in the store's place");
        let error = fixture
            .run(&DashboardCommand::RemoveCredential {
                connector_id: "some-provider".into(),
            })
            .expect_err("unreadable store");
        assert!(format!("{error:#}").contains("reading credential state"));
    }

    #[tokio::test]
    async fn disconnecting_a_project_forgets_its_grants_before_dropping_the_id() {
        let fixture = Fixture::new();
        let policy_path = fixture.project_policy(&["some-provider", "some-other"]);
        let project = fixture.seed_grant(&policy_path, "some-provider");
        let run_id = register_run(Some(&policy_path), &[]);

        let notice = fixture
            .run(&DashboardCommand::DisconnectProject {
                connector_id: "some-provider".into(),
                sandbox_id: run_id.clone(),
            })
            .expect("disconnect succeeds");

        assert!(notice.starts_with("some-provider was disconnected from "));
        let policy = lns_policy::Policy::load_or_default(&policy_path).expect("reload policy");
        assert_eq!(policy.connectors, ["some-other"]);
        let grants = fixture.grants();
        assert!(grants.grants.is_empty());
        assert_eq!(grants.revocations_of(&project, "some-provider"), 1);
        crate::run_registry::cancel(&run_id);
    }

    #[tokio::test]
    async fn a_credential_the_definition_declares_cannot_be_disconnected() {
        let fixture = Fixture::new();
        let policy_path = fixture.project_policy(&["some-provider"]);
        let run_id = register_run(Some(&policy_path), &["some-provider"]);

        let error = fixture
            .run(&DashboardCommand::DisconnectProject {
                connector_id: "some-provider".into(),
                sandbox_id: run_id.clone(),
            })
            .expect_err("declared by the definition");

        assert!(error.to_string().contains("cannot be revoked"));
        let policy = lns_policy::Policy::load_or_default(&policy_path).expect("reload policy");
        assert_eq!(policy.connectors, ["some-provider"]);
        crate::run_registry::cancel(&run_id);
    }

    #[tokio::test]
    async fn disconnecting_needs_a_live_sandbox_with_a_project_policy() {
        let fixture = Fixture::new();
        let gone = fixture
            .run(&DashboardCommand::DisconnectProject {
                connector_id: "some-provider".into(),
                sandbox_id: "missing".into(),
            })
            .expect_err("no such sandbox");
        assert!(format!("{gone:#}").contains("no longer available"));

        let run_id = register_run(None, &[]);
        let no_policy = fixture
            .run(&DashboardCommand::DisconnectProject {
                connector_id: "some-provider".into(),
                sandbox_id: run_id.clone(),
            })
            .expect_err("no project policy");
        assert!(format!("{no_policy:#}").contains("no project policy"));
        crate::run_registry::cancel(&run_id);
    }

    #[tokio::test]
    async fn disconnecting_a_connector_the_project_never_had_changes_nothing() {
        let fixture = Fixture::new();
        let policy_path = fixture.project_policy(&["some-other"]);
        let run_id = register_run(Some(&policy_path), &[]);

        let error = fixture
            .run(&DashboardCommand::DisconnectProject {
                connector_id: "some-provider".into(),
                sandbox_id: run_id.clone(),
            })
            .expect_err("not connected");

        assert!(error.to_string().contains("no longer connected"));
        assert_eq!(
            lns_policy::Policy::load_or_default(&policy_path)
                .expect("reload policy")
                .connectors,
            ["some-other"]
        );
        crate::run_registry::cancel(&run_id);
    }

    fn register_run(policy_path: Option<&Path>, credential_slots: &[&str]) -> String {
        let run_id = crate::run_registry::allocate_run_id();
        let (cancel_tx, _cancel_rx) = tokio::sync::oneshot::channel();
        crate::run_registry::register(
            run_id.clone(),
            crate::run_registry::RunHandle {
                cancel_tx,
                detach_tx: std::sync::Mutex::new(None),
                task: tokio::spawn(async {}),
                input_tx: None,
                connector: None,
                name: "calm-finch".into(),
                image: "example:latest".into(),
                command: "some-command".into(),
                started: "2026-01-01T00:00:00Z".into(),
                status: std::sync::Mutex::new(lns_ipc::RunStatus::Running),
                logs: std::sync::Arc::new(crate::run_log::RunLogBuffer::default()),
                config: lns_ipc::RunConfig {
                    policy_path: policy_path.map(|path| path.display().to_string()),
                    ..lns_ipc::RunConfig::default()
                },
                credential_slots: credential_slots
                    .iter()
                    .map(|name| lns_artifact::spec::CredentialSlot {
                        name: (*name).to_string(),
                        env: "SOME_TOKEN".into(),
                        required: true,
                    })
                    .collect(),
            },
        );
        run_id
    }

    #[test]
    fn the_default_stores_are_the_per_machine_files() {
        let stores = CommandStores::default_paths();
        assert_eq!(
            stores.credentials,
            lns_policy::credentials::default_credentials_path()
        );
        assert_eq!(
            stores.grants,
            lns_policy::grants::default_workload_grants_path()
        );
    }

    #[test]
    fn a_replace_reaches_the_entry_through_the_state_file() {
        let mut state = CredentialStateFile::from(HashMap::from([(
            "some-provider".to_string(),
            CredentialEntry::Stored {
                value: "old-secret".into(),
            },
        )]));
        replace_saved_credential(&mut state, "some-provider", "new-secret").expect("replace");
        assert_eq!(
            state.get("some-provider"),
            Some(&CredentialEntry::Stored {
                value: "new-secret".into()
            })
        );
    }
}
