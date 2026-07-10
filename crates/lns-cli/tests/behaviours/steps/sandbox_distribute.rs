use cucumber::{given, then};
use lns_ipc::{Request, Response};

use crate::world::BehaviourWorld;

#[given("the registry accepts the push")]
fn registry_accepts_push(w: &mut BehaviourWorld) {
    w.push_outcome = Some(Ok(format!("sha256:{}", "a".repeat(64))));
}

#[given("the stored credential for the registry lacks push scope")]
fn credential_lacks_push_scope(w: &mut BehaviourWorld) {
    w.push_outcome = Some(Err("credential for ghcr.io lacks push scope".to_string()));
}

#[given(regex = r#"^the sandbox "([^"]+)" is cached$"#)]
fn sandbox_is_cached(w: &mut BehaviourWorld, reference: String) {
    w.sandbox.response = Some(Response::ImageTagged {
        from: reference.clone(),
        to: reference,
    });
}

#[then("the service received a request to pull the base image")]
fn service_received_pull(w: &mut BehaviourWorld) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests
        .iter()
        .any(|r| matches!(r, Request::PullImage { .. }))
    {
        Ok(())
    } else {
        Err(format!("expected a PullImage request among {requests:?}"))
    }
}

#[then(regex = r#"^the sandbox "([^"]+)" resolves to the same cached artifact$"#)]
fn sandbox_resolves_to_same_artifact(w: &mut BehaviourWorld, to: String) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests
        .iter()
        .any(|r| matches!(r, Request::TagImage { to: t, .. } if *t == to))
    {
        Ok(())
    } else {
        Err(format!(
            "expected a TagImage request tagging {to:?} among {requests:?}"
        ))
    }
}
