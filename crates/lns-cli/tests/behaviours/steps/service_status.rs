use anyhow::Result;
use cucumber::{given, then, when};
use std::time::Duration;

use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use lns_cli::service::client::BoxFuture;
use lns_cli::service::{ServiceClient, cmd_status};
use lns_ipc::{CredentialBackendKind, StatusInfo};

/// Stands in for the running service, answering only the status probe.
struct StatusOnlyClient {
    backend: Option<CredentialBackendKind>,
}

impl ServiceClient for StatusOnlyClient {
    fn ping(&self) -> BoxFuture<'_, bool> {
        Box::pin(async { true })
    }

    fn status(&self) -> BoxFuture<'_, Option<StatusInfo>> {
        let info = StatusInfo {
            pid: 4242,
            uptime_secs: 17,
            version: "test-version".into(),
            credential_backend: self.backend,
        };
        Box::pin(async move { Some(info) })
    }

    fn shutdown(&self) -> BoxFuture<'_, Option<()>> {
        Box::pin(async { None })
    }

    fn start_and_wait_for_ready(&self, _timeout: Duration) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn wait_for_ready(&self, _timeout: Duration) -> BoxFuture<'_, bool> {
        Box::pin(async { true })
    }

    fn wait_for_stopped(&self, _timeout: Duration) -> BoxFuture<'_, bool> {
        Box::pin(async { true })
    }

    fn cancel_run(&self, _run_id: String) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

#[given("the background service reports the OS keychain credential backend")]
fn given_keychain_backend(world: &mut BehaviourWorld) {
    world.status_backend = Some(CredentialBackendKind::OsKeychain);
}

#[given("the background service reports the plaintext file credential backend")]
fn given_plaintext_backend(world: &mut BehaviourWorld) {
    world.status_backend = Some(CredentialBackendKind::PlaintextFile);
}

#[when(regex = r#"^the developer runs "lns service status"$"#)]
async fn run_service_status(world: &mut BehaviourWorld) {
    let client = StatusOnlyClient {
        backend: world.status_backend,
    };
    let mut buf = Vec::<u8>::new();
    let run = match cmd_status(&client, &mut buf).await {
        Ok(()) => CliRun {
            exit_code: 0,
            output: String::from_utf8_lossy(&buf).into_owned(),
        },
        Err(e) => CliRun {
            exit_code: 1,
            output: format!("{e:#}"),
        },
    };
    world.result = Some(run);
}

#[then("the status output notes credential values are stored in the OS keychain")]
fn status_notes_keychain(world: &mut BehaviourWorld) {
    let out = &world
        .result
        .as_ref()
        .expect("a run must have happened")
        .output;
    assert!(
        out.contains("Credentials: stored in the OS keychain"),
        "expected the keychain line, got: {out}"
    );
}

#[then("the status output notes credential values are stored in a plaintext file")]
fn status_notes_plaintext(world: &mut BehaviourWorld) {
    let out = &world
        .result
        .as_ref()
        .expect("a run must have happened")
        .output;
    assert!(
        out.contains("Credentials: stored in a plaintext file"),
        "expected the plaintext line, got: {out}"
    );
}
