wit_bindgen::generate!({ world: "mechanism", path: "../../../crates/lns-service/wit" });

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use exports::lns::connector::adapter::Guest;
use lns::connector::types::{Answer, Ask, Outcome, Step};
use lns::connector::{browser, entropy, http};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const RESOURCE: &str = "https://mcp.linear.app/mcp";
const METADATA: &str = "https://mcp.linear.app/.well-known/oauth-authorization-server";
const API: &str = "https://api.linear.app/graphql";

struct Linear;

#[derive(Serialize, Deserialize)]
enum Waiting {
    Start,
    Browser {
        handle: String,
        verifier: String,
        client_id: String,
        redirect_uri: String,
    },
}

impl Guest for Linear {
    fn connect(_now: u64) -> Step {
        ask(
            "Connect to Linear with read-only access. LNS will open your browser, then verify that the same credential works with Linear MCP and the direct API.",
            Waiting::Start,
        )
    }

    fn resume(state: Vec<u8>, _answers: Vec<Answer>, now: u64) -> Step {
        match serde_json::from_slice(&state) {
            Ok(Waiting::Start) => start().unwrap_or_else(Step::Failed),
            Ok(Waiting::Browser {
                handle,
                verifier,
                client_id,
                redirect_uri,
            }) => {
                finish(handle, verifier, client_id, redirect_uri, now).unwrap_or_else(Step::Failed)
            }
            Err(_) => Step::Failed("This sign-in could not be resumed. Connect again.".into()),
        }
    }

    fn refresh(values: Vec<Answer>, now: u64) -> Result<Outcome, String> {
        let client_id = value(&values, "client_id")?;
        let refresh_token = value(&values, "refresh_token")?;
        let response = token(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
            ("resource", RESOURCE),
        ])?;
        outcome(response, client_id, Some(refresh_token), now)
    }

    fn revoke(_values: Vec<Answer>, _now: u64) -> Result<(), String> {
        Err("Remove the LNS authorization in Linear's application settings to revoke provider access.".into())
    }
}

fn start() -> Result<Step, String> {
    let metadata = json_request("GET", METADATA, Vec::new(), Vec::new())?;
    for (field, expected) in [
        ("issuer", "https://mcp.linear.app"),
        ("authorization_endpoint", "https://mcp.linear.app/authorize"),
        ("token_endpoint", "https://mcp.linear.app/token"),
        ("registration_endpoint", "https://mcp.linear.app/register"),
    ] {
        if metadata.get(field).and_then(serde_json::Value::as_str) != Some(expected) {
            return Err(
                "Linear's OAuth metadata changed. Update this connector before signing in.".into(),
            );
        }
    }
    let session = browser::prepare().map_err(|_| "LNS could not prepare browser authorization.")?;
    let registered = json_request(
        "POST",
        "https://mcp.linear.app/register",
        json_headers(),
        serde_json::json!({
            "client_name": "LNS", "redirect_uris": [session.redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"], "response_types": ["code"],
            "token_endpoint_auth_method": "none"
        })
        .to_string()
        .into_bytes(),
    )?;
    let client_id = string(&registered, "client_id")?.to_string();
    let verifier = URL_SAFE_NO_PAD.encode(entropy::bytes(32));
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = url::Url::parse("https://mcp.linear.app/authorize")
        .map_err(|_| "Invalid authorization endpoint.")?;
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", client_id.as_str()),
        ("redirect_uri", session.redirect_uri.as_str()),
        ("state", session.state.as_str()),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("scope", "read"),
        ("resource", RESOURCE),
    ]);
    browser::open(&session.handle, url.as_str())
        .map_err(|_| "LNS could not open Linear authorization.")?;
    Ok(ask(
        "Finish signing in to Linear in your browser, then continue here.",
        Waiting::Browser {
            handle: session.handle,
            verifier,
            client_id,
            redirect_uri: session.redirect_uri,
        },
    ))
}

fn finish(
    handle: String,
    verifier: String,
    client_id: String,
    redirect_uri: String,
    now: u64,
) -> Result<Step, String> {
    let code = match browser::poll(&handle) {
        Ok(Some(code)) => code,
        Ok(None) => {
            return Ok(ask(
                "Linear authorization is still waiting in your browser. Continue after approving it.",
                Waiting::Browser {
                    handle,
                    verifier,
                    client_id,
                    redirect_uri,
                },
            ));
        }
        Err(_) => return Err("Linear authorization ended or expired. Connect again.".into()),
    };
    let response = token(&[
        ("grant_type", "authorization_code"),
        ("client_id", &client_id),
        ("redirect_uri", &redirect_uri),
        ("code", &code),
        ("code_verifier", &verifier),
        ("resource", RESOURCE),
    ])?;
    let access_token = string(&response, "access_token")?;
    verify_access(access_token)?;
    Ok(Step::Done(outcome(response, &client_id, None, now)?))
}

fn verify_access(access_token: &str) -> Result<(), String> {
    let mut headers = json_headers();
    headers.push(("Authorization".into(), format!("Bearer {access_token}")));
    let api = json_request(
        "POST",
        API,
        headers.clone(),
        br#"{"query":"{ viewer { id } }"}"#.to_vec(),
    )
    .map_err(|reason| {
        format!("The direct API check failed: {reason} No general-purpose connection was saved.")
    })?;
    if api
        .pointer("/data/viewer/id")
        .and_then(serde_json::Value::as_str)
        .is_none()
        || api.get("errors").is_some()
    {
        return Err("Linear's MCP sign-in did not authorize the direct API. No general-purpose connection was saved.".into());
    }
    headers.push((
        "Accept".into(),
        "application/json, text/event-stream".into(),
    ));
    let mcp = json_request("POST", RESOURCE, headers, serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": { "name": "lns", "version": "0.1.0" } }
    }).to_string().into_bytes())?;
    if mcp
        .pointer("/result/protocolVersion")
        .and_then(serde_json::Value::as_str)
        .is_none()
    {
        return Err("Linear did not confirm MCP access. No connection was saved.".into());
    }
    Ok(())
}

fn outcome(
    response: serde_json::Value,
    client_id: &str,
    previous_refresh: Option<&str>,
    now: u64,
) -> Result<Outcome, String> {
    let access = string(&response, "access_token")?;
    if !string(&response, "token_type")?.eq_ignore_ascii_case("bearer") {
        return Err("Linear returned an unsupported token type.".into());
    }
    let refresh = response
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .filter(|v| !v.is_empty())
        .or(previous_refresh)
        .ok_or("Linear returned no refresh token.")?;
    let lifetime = response
        .get("expires_in")
        .and_then(serde_json::Value::as_u64)
        .filter(|v| *v > 0)
        .and_then(|v| v.checked_mul(1000))
        .and_then(|v| now.checked_add(v))
        .ok_or("Linear returned no valid token lifetime.")?;
    let authority = response
        .get("scope")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .split_whitespace()
        .map(str::to_string)
        .collect();
    Ok(Outcome {
        values: vec![
            Answer {
                name: "access_token".into(),
                value: access.into(),
            },
            Answer {
                name: "refresh_token".into(),
                value: refresh.into(),
            },
            Answer {
                name: "client_id".into(),
                value: client_id.into(),
            },
        ],
        authority,
        expires_at_millis: Some(lifetime),
    })
}

fn token(params: &[(&str, &str)]) -> Result<serde_json::Value, String> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params.iter().copied())
        .finish();
    json_request(
        "POST",
        "https://mcp.linear.app/token",
        vec![(
            "Content-Type".into(),
            "application/x-www-form-urlencoded".into(),
        )],
        body.into_bytes(),
    )
}

fn json_headers() -> Vec<(String, String)> {
    vec![("Content-Type".into(), "application/json".into())]
}

fn json_request(
    method: &str,
    url: &str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
) -> Result<serde_json::Value, String> {
    let response = http::fetch(&http::Request {
        method: method.into(),
        url: url.into(),
        headers,
        body,
    })
    .map_err(|_| "A call to Linear did not finish.".to_string())?;
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "Linear refused the request (HTTP {}).",
            response.status
        ));
    }
    if let Ok(value) = serde_json::from_slice(&response.body) {
        return Ok(value);
    }
    let text = std::str::from_utf8(&response.body)
        .map_err(|_| "Linear returned an unreadable response.")?;
    text.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .find_map(|line| serde_json::from_str(line.trim()).ok())
        .ok_or("Linear returned an unreadable response.".into())
}

fn string<'a>(object: &'a serde_json::Value, key: &str) -> Result<&'a str, String> {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Linear's response omitted {key}."))
}

fn value<'a>(values: &'a [Answer], key: &str) -> Result<&'a str, String> {
    values
        .iter()
        .find(|v| v.name == key)
        .map(|v| v.value.as_str())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("This connection has no {key}."))
}

fn ask(message: &str, state: Waiting) -> Step {
    match serde_json::to_vec(&state) {
        Ok(state) => Step::Ask(Ask {
            message: message.into(),
            fields: Vec::new(),
            state,
        }),
        Err(_) => Step::Failed("Could not prepare this sign-in step.".into()),
    }
}

export!(Linear);
