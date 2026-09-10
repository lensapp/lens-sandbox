//! GitHub's device flow (RFC 8628), written as a mechanism lns cannot read.
//!
//! The flow lns lends no listener, so the redirect leg is a device code the
//! user confirms in a browser. Nothing here reaches anything but `github.com`,
//! and nothing decides that but the method that declared this component.

wit_bindgen::generate!({ world: "mechanism", path: "../../../crates/lns-service/wit" });

use exports::lns::connector::adapter::Guest;
use lns::connector::http::{Request, fetch};
use lns::connector::types::{Answer, Ask, CallError, Field, Outcome, Step};
use serde::{Deserialize, Serialize};

/// The Client ID of a GitHub App the connector's own author registered. Empty here, because no App backs this example, so each user signs in through a registration of their own.
const OUR_CLIENT_ID: &str = "";

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

struct Github;

/// What one round left for the next. lns holds this in memory and hands it back verbatim; it never reaches a disk.
#[derive(Serialize, Deserialize)]
enum Waiting {
    ForAClientId,
    ForTheUser(Underway),
}

/// A flow GitHub has already named a code for, which is the only state there is anything to wait on.
#[derive(Serialize, Deserialize)]
struct Underway {
    client_id: String,
    device_code: String,
    user_code: String,
    verification_uri: String,
}

#[derive(Deserialize)]
struct Named {
    device_code: String,
    user_code: String,
    verification_uri: String,
}

#[derive(Deserialize)]
struct Answered {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
    error: Option<String>,
}

impl Guest for Github {
    fn connect(_now_millis: u64) -> Step {
        match OUR_CLIENT_ID {
            "" => ask_for_a_client_id(),
            ours => show_a_code(ours.to_string()),
        }
    }

    fn resume(state: Vec<u8>, answers: Vec<Answer>, now_millis: u64) -> Step {
        let Some(waiting) = recall(&state) else {
            return Step::Failed(
                "this component could not read the state it was resumed with".to_string(),
            );
        };
        match waiting {
            Waiting::ForAClientId => match named("client_id", &answers) {
                Some(client_id) if !client_id.trim().is_empty() => {
                    show_a_code(client_id.trim().to_string())
                }
                _ => Step::Failed(
                    "the device flow starts from a Client ID, and none was given".to_string(),
                ),
            },
            Waiting::ForTheUser(underway) => ask_github_whether_yet(underway, now_millis),
        }
    }

    /// GitHub rotates both tokens, so a renewal that kept the old refresh token would work once and then never again. It needs no client secret, because this token came from the device flow.
    fn refresh(values: Vec<Answer>, now_millis: u64) -> Result<Outcome, String> {
        let client_id = must_hold("client_id", &values)?;
        let refresh_token = must_hold("refresh_token", &values)?;
        let answered = ask_for_a_token(&serde_json::json!({
            "client_id": client_id,
            "refresh_token": refresh_token,
            "grant_type": "refresh_token",
        }))?;
        if let Some(refused) = answered.error {
            return Err(format!("GitHub would not renew it: {refused}"));
        }
        produced(client_id, answered, now_millis)
    }

    /// GitHub's revocation API authenticates with the App's client secret, which a component the user runs cannot hold. Saying so is truer than reporting a revocation that did not happen.
    fn revoke(_values: Vec<Answer>, _now_millis: u64) -> Result<(), String> {
        Err(
            "GitHub lets only the App's own server revoke a token. Remove this connector's authorization at https://github.com/settings/applications."
                .to_string(),
        )
    }
}

fn ask_for_a_client_id() -> Step {
    match held(&Waiting::ForAClientId) {
        Ok(state) => Step::Ask(Ask {
            message: "Register a GitHub App with the device flow enabled and user tokens set to expire, then paste its Client ID.".to_string(),
            fields: vec![Field {
                name: "client_id".to_string(),
                label: "GitHub App Client ID".to_string(),
                secret: false,
            }],
            state,
        }),
        Err(why) => Step::Failed(why),
    }
}

/// The first leg: GitHub names a code, and the user confirms it in a browser this component never sees.
fn show_a_code(client_id: String) -> Step {
    // No `scope`: a GitHub App draws its access from the permissions it was installed with, and ignores a scope asked for here.
    let body = match post(
        DEVICE_CODE_URL,
        &serde_json::json!({ "client_id": client_id }),
    ) {
        Ok(body) => body,
        Err(why) => return Step::Failed(why),
    };
    match serde_json::from_slice::<Named>(&body) {
        Ok(named) => wait_on(Underway {
            client_id,
            device_code: named.device_code,
            user_code: named.user_code,
            verification_uri: named.verification_uri,
        }),
        Err(_) => Step::Failed(
            "GitHub named no device code. Check that the Client ID is the App's and that the App has the device flow enabled.".to_string(),
        ),
    }
}

/// A round that shows the user where to go and collects nothing, which is how a device flow waits when it cannot sleep.
fn wait_on(underway: Underway) -> Step {
    let message = format!(
        "Open {} and enter the code {}. Continue here once GitHub has accepted it.",
        underway.verification_uri, underway.user_code
    );
    match held(&Waiting::ForTheUser(underway)) {
        Ok(state) => Step::Ask(Ask {
            message,
            fields: Vec::new(),
            state,
        }),
        Err(why) => Step::Failed(why),
    }
}

/// One ask of the token endpoint per round. The component is lent no clock to sleep on, so the user's own press is what paces the poll.
fn ask_github_whether_yet(underway: Underway, now_millis: u64) -> Step {
    let answered = match ask_for_a_token(&serde_json::json!({
        "client_id": underway.client_id,
        "device_code": underway.device_code,
        "grant_type": DEVICE_GRANT,
    })) {
        Ok(answered) => answered,
        Err(why) => return Step::Failed(why),
    };
    match answered.error.as_deref() {
        None => match produced(underway.client_id.clone(), answered, now_millis) {
            Ok(outcome) => Step::Done(outcome),
            Err(why) => Step::Failed(why),
        },
        Some("authorization_pending" | "slow_down") => wait_on(underway),
        Some("expired_token") => {
            Step::Failed("the code ran out before GitHub was told to accept it".to_string())
        }
        Some("access_denied") => Step::Failed("the request was declined at GitHub".to_string()),
        Some(other) => Step::Failed(format!("GitHub refused the code: {other}")),
    }
}

/// Every output the method declares, or nothing: lns stores a connection whole or not at all, and a connection missing its refresh token could never be renewed.
fn produced(client_id: String, answered: Answered, now_millis: u64) -> Result<Outcome, String> {
    let access_token = answered
        .access_token
        .ok_or_else(|| "GitHub accepted the code and returned no access token".to_string())?;
    let refresh_token = answered.refresh_token.ok_or_else(|| {
        "GitHub returned no refresh token, so this connection could never be renewed. Turn on \"Expire user authorization tokens\" in the App's settings and connect again."
            .to_string()
    })?;
    Ok(Outcome {
        values: vec![
            answer("access_token", access_token),
            answer("refresh_token", refresh_token),
            answer("client_id", client_id),
        ],
        authority: granted(answered.scope.as_deref()),
        // Saturating, because a lifetime that wrapped would read as a token that expired before it was issued.
        expires_at_millis: answered
            .expires_in
            .map(|run| now_millis.saturating_add(run.saturating_mul(1000))),
    })
}

fn ask_for_a_token(body: &serde_json::Value) -> Result<Answered, String> {
    let answered = post(TOKEN_URL, body)?;
    serde_json::from_slice(&answered)
        .map_err(|_| "GitHub's answer at the token endpoint was not one this reads".to_string())
}

/// Never carries the answer's body into its error: that body is where a token would be.
fn post(url: &str, body: &serde_json::Value) -> Result<Vec<u8>, String> {
    let sending =
        serde_json::to_vec(body).map_err(|_| "the request would not encode".to_string())?;
    let request = Request {
        method: "POST".to_string(),
        url: url.to_string(),
        headers: vec![
            ("accept".to_string(), "application/json".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
            ("user-agent".to_string(), "lns-connector-github".to_string()),
        ],
        body: sending,
    };
    match fetch(&request) {
        Ok(answered) if (200..300).contains(&answered.status) => Ok(answered.body),
        Ok(answered) => Err(format!("GitHub answered {} at {url}", answered.status)),
        Err(CallError::Refused(why)) => Err(format!("lns refused a call to {url}: {why}")),
        Err(CallError::Failed(why)) => Err(format!("a call to {url} did not finish: {why}")),
    }
}

/// What GitHub says it granted. A GitHub App reports none, because its access is the App's own permissions rather than anything asked for here.
fn granted(scope: Option<&str>) -> Vec<String> {
    scope
        .unwrap_or_default()
        .split([',', ' '])
        .filter(|held| !held.is_empty())
        .map(str::to_string)
        .collect()
}

fn answer(name: &str, value: String) -> Answer {
    Answer {
        name: name.to_string(),
        value,
    }
}

fn named(name: &str, answers: &[Answer]) -> Option<String> {
    answers
        .iter()
        .find(|answer| answer.name == name)
        .map(|answer| answer.value.clone())
}

fn must_hold(name: &str, values: &[Answer]) -> Result<String, String> {
    named(name, values).ok_or_else(|| format!("the connection holds no {name} to renew from"))
}

fn held(waiting: &Waiting) -> Result<Vec<u8>, String> {
    serde_json::to_vec(waiting)
        .map_err(|_| "this component could not record what it was waiting for".to_string())
}

fn recall(state: &[u8]) -> Option<Waiting> {
    serde_json::from_slice(state).ok()
}

export!(Github);
