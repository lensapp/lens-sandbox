//! GitHub's device flow (RFC 8628), written as a mechanism lns cannot read.
//!
//! The flow lns lends no listener, so the redirect leg is a device code the
//! user confirms in a browser. Nothing here reaches anything but `github.com`,
//! and nothing decides that but the method that declared this component.
//!
//! One build serves one method, because a method declares what it produces and
//! a component is told nothing about which one invoked it. A GitHub App issues
//! a token that expires and can be renewed; an OAuth App issues one that never
//! does and never can.

#[cfg(not(any(feature = "sign-in", feature = "oauth-sign-in")))]
compile_error!("build with --features sign-in or --features oauth-sign-in");

#[cfg(all(feature = "sign-in", feature = "oauth-sign-in"))]
compile_error!("one variant per build: each method declares its own outputs");

wit_bindgen::generate!({ world: "mechanism", path: "../../../crates/lns-service/wit" });

use exports::lns::connector::adapter::Guest;
use lns::connector::http::{Request, fetch};
use lns::connector::types::{Answer, Ask, CallError, Field, Outcome, Step};
use serde::{Deserialize, Serialize};

/// The Client ID of an app the connector's own author registered. Empty here, because no app backs this example, so each user signs in through a registration of their own.
const OUR_CLIENT_ID: &str = "";

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

struct Github;

/// What one round left for the next. lns holds this in memory and hands it back verbatim; it never reaches a disk.
#[derive(Serialize, Deserialize)]
enum Waiting {
    ForWhatToSignInWith,
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
        let asking = fields_to_ask();
        if asking.is_empty() {
            return show_a_code(OUR_CLIENT_ID.to_string(), None);
        }
        match held(&Waiting::ForWhatToSignInWith) {
            Ok(state) => Step::Ask(Ask {
                message: FIRST_ROUND.to_string(),
                fields: asking,
                state,
            }),
            Err(why) => Step::Failed(why),
        }
    }

    fn resume(state: Vec<u8>, answers: Vec<Answer>, now_millis: u64) -> Step {
        let Some(waiting) = recall(&state) else {
            return Step::Failed(
                "this component could not read the state it was resumed with".to_string(),
            );
        };
        match waiting {
            Waiting::ForWhatToSignInWith => match signing_in_with(&answers) {
                Ok(client_id) => show_a_code(client_id, asked_scope(&answers)),
                Err(why) => Step::Failed(why),
            },
            Waiting::ForTheUser(underway) => ask_github_whether_yet(underway, now_millis),
        }
    }

    fn refresh(values: Vec<Answer>, now_millis: u64) -> Result<Outcome, String> {
        renew(values, now_millis)
    }

    /// GitHub's revocation API authenticates with the app's client secret, which a component the user runs cannot hold. Saying so is truer than reporting a revocation that did not happen.
    fn revoke(_values: Vec<Answer>, _now_millis: u64) -> Result<(), String> {
        Err(
            "GitHub lets only the app's own server revoke a token. Remove this connector's authorization at https://github.com/settings/applications."
                .to_string(),
        )
    }
}

/// The Client ID to sign in with: the one this build was compiled around, or the one the user just gave it.
fn signing_in_with(answers: &[Answer]) -> Result<String, String> {
    if !OUR_CLIENT_ID.is_empty() {
        return Ok(OUR_CLIENT_ID.to_string());
    }
    match named("client_id", answers) {
        Some(given) if !given.trim().is_empty() => Ok(given.trim().to_string()),
        _ => Err("the device flow starts from a Client ID, and none was given".to_string()),
    }
}

fn a_client_id_field() -> Vec<Field> {
    if OUR_CLIENT_ID.is_empty() {
        return vec![Field {
            name: "client_id".to_string(),
            label: CLIENT_ID_LABEL.to_string(),
            secret: false,
        }];
    }
    Vec::new()
}

#[cfg(feature = "sign-in")]
const FIRST_ROUND: &str = "Register a GitHub App with the device flow enabled, then paste its Client ID. Leave \"User-to-server token expiration\" opted in — this method renews itself, and cannot do that without it.";

#[cfg(feature = "sign-in")]
const CLIENT_ID_LABEL: &str = "GitHub App Client ID";

#[cfg(feature = "sign-in")]
fn fields_to_ask() -> Vec<Field> {
    a_client_id_field()
}

/// A GitHub App draws its access from the permissions it was installed with, and ignores a scope asked for here.
#[cfg(feature = "sign-in")]
fn asked_scope(_answers: &[Answer]) -> Option<String> {
    None
}

#[cfg(feature = "oauth-sign-in")]
const FIRST_ROUND: &str = "Register an OAuth App with the device flow enabled, then paste its Client ID and the scopes you want this token to carry.";

#[cfg(feature = "oauth-sign-in")]
const CLIENT_ID_LABEL: &str = "OAuth App Client ID";

#[cfg(feature = "oauth-sign-in")]
fn fields_to_ask() -> Vec<Field> {
    let mut asking = a_client_id_field();
    asking.push(Field {
        name: "scope".to_string(),
        label: "scopes, space-separated — e.g. \"public_repo read:user\", or empty for public read-only".to_string(),
        secret: false,
    });
    asking
}

/// Unlike a GitHub App, an OAuth App's token carries the scopes it was asked for, so this is the user's to decide and never this component's.
#[cfg(feature = "oauth-sign-in")]
fn asked_scope(answers: &[Answer]) -> Option<String> {
    named("scope", answers)
        .map(|asked| asked.trim().to_string())
        .filter(|asked| !asked.is_empty())
}

/// The first leg: GitHub names a code, and the user confirms it in a browser this component never sees.
fn show_a_code(client_id: String, scope: Option<String>) -> Step {
    let mut asking = serde_json::json!({ "client_id": client_id });
    if let Some(scope) = scope {
        asking["scope"] = serde_json::Value::String(scope);
    }
    let body = match post(DEVICE_CODE_URL, &asking) {
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
            "GitHub named no device code. Check that the Client ID is the app's and that the app has the device flow enabled.".to_string(),
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
#[cfg(feature = "sign-in")]
fn produced(client_id: String, answered: Answered, now_millis: u64) -> Result<Outcome, String> {
    let access_token = answered
        .access_token
        .ok_or_else(|| "GitHub accepted the code and returned no access token".to_string())?;
    let refresh_token = answered.refresh_token.ok_or_else(|| {
        "GitHub returned no refresh token, so this connection could never be renewed. This method needs a GitHub App with \"User-to-server token expiration\" opted in — an OAuth App never returns one, and signs in through the oauth-sign-in method instead."
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

/// This method declares no refresh token, so every renewal of a token that expires would fail. One is refused rather than stored, which sends the user to the method that can renew instead of to a connection that quietly stops working.
#[cfg(feature = "oauth-sign-in")]
fn produced(_client_id: String, answered: Answered, _now_millis: u64) -> Result<Outcome, String> {
    let access_token = answered
        .access_token
        .ok_or_else(|| "GitHub accepted the code and returned no access token".to_string())?;
    if answered.refresh_token.is_some() || answered.expires_in.is_some() {
        return Err(
            "GitHub returned a token that expires, which this method cannot renew: it keeps no refresh token. Register a GitHub App and sign in with the sign-in method instead."
                .to_string(),
        );
    }
    Ok(Outcome {
        values: vec![answer("access_token", access_token)],
        authority: granted(answered.scope.as_deref()),
        expires_at_millis: None,
    })
}

/// GitHub rotates both tokens, so a renewal that kept the old refresh token would work once and then never again. It needs no client secret, because this token came from the device flow.
#[cfg(feature = "sign-in")]
fn renew(values: Vec<Answer>, now_millis: u64) -> Result<Outcome, String> {
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

/// Nothing calls this on a schedule, because this method reports no expiry. It answers rather than reaching GitHub, so a caller that asked anyway spends no call on it.
#[cfg(feature = "oauth-sign-in")]
fn renew(_values: Vec<Answer>, _now_millis: u64) -> Result<Outcome, String> {
    Err("an OAuth App's token does not expire, and GitHub will not renew one".to_string())
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

/// What GitHub says it granted, which is not always what was asked for — and nothing at all for a GitHub App, whose access is the App's own permissions.
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

#[cfg(feature = "sign-in")]
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
