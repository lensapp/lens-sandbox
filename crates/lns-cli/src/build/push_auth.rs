use lns_policy::registry_auth::{RegistryAuthFile, credential_for};
use oci_client::errors::{OciDistributionError, OciEnvelope, OciErrorCode};
use oci_client::{Reference, secrets::RegistryAuth};

/// Pick the push credential for `registry` from a loaded auth file, falling back to anonymous when the store couldn't be read or holds no entry for it.
pub(crate) fn select_auth(
    loaded: std::io::Result<RegistryAuthFile>,
    registry: &str,
) -> RegistryAuth {
    let Ok(file) = loaded else {
        return RegistryAuth::Anonymous;
    };
    match credential_for(&file, registry) {
        Some(cred) => RegistryAuth::Basic(cred.username.clone(), cred.secret.clone()),
        None => RegistryAuth::Anonymous,
    }
}

enum Refusal {
    LoginCanFix(Option<String>),
    Denied(String),
    Unavailable(String),
}

/// A 401/403 from any push phase is a refusal — bearer registries hand `auth()` a reduced-scope token and only refuse at upload — so it is answered here; every other failure surfaces verbatim.
fn refusal(err: &OciDistributionError) -> Option<Refusal> {
    match err {
        OciDistributionError::AuthenticationFailure(body) => Some(token_body_refusal(body)),
        OciDistributionError::UnauthorizedError { .. } => Some(Refusal::LoginCanFix(None)),
        OciDistributionError::ServerError {
            code: 401, message, ..
        } => Some(Refusal::LoginCanFix(registry_words(message))),
        OciDistributionError::ServerError {
            code: 403, message, ..
        } => Some(match registry_words(message) {
            Some(words) => Refusal::Denied(words),
            None => Refusal::LoginCanFix(None),
        }),
        OciDistributionError::RegistryError { envelope, .. } => {
            envelope_words(envelope).map(Refusal::Denied)
        }
        _ => None,
    }
}

/// `auth()` collapses every non-200 from `/token` into a bodyless-or-not string, so the body is all lns has to tell an outage from a refusal.
fn token_body_refusal(body: &str) -> Refusal {
    match registry_words(body) {
        None => Refusal::LoginCanFix(None),
        Some(words) if reports_server_status(body) => Refusal::Unavailable(words),
        Some(words) => Refusal::Denied(words),
    }
}

fn registry_words(body: &str) -> Option<String> {
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    if let Ok(envelope) = serde_json::from_str::<OciEnvelope>(body)
        && let Some(words) = envelope_words(&envelope)
    {
        return Some(words);
    }
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(json) => Some(explained_field(&json).unwrap_or_else(|| body.to_string())),
        Err(_) => Some(body.to_string()),
    }
}

fn explained_field(json: &serde_json::Value) -> Option<String> {
    json.get("details")
        .or_else(|| json.get("message"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn envelope_words(envelope: &OciEnvelope) -> Option<String> {
    let lines: Vec<String> = envelope
        .errors
        .iter()
        .map(|error| match error.message.trim() {
            "" => code_label(&error.code),
            message => format!("{}: {message}", code_label(&error.code)),
        })
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(lines.join("\n"))
}

fn code_label(code: &OciErrorCode) -> String {
    let named = format!("{code:?}");
    let mut label = String::with_capacity(named.len() + 4);
    for (position, letter) in named.char_indices() {
        if position != 0 && letter.is_uppercase() {
            label.push('_');
        }
        label.push(letter.to_ascii_uppercase());
    }
    label
}

fn reports_server_status(body: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(json) => status_field(&json).is_some_and(is_server_status),
        Err(_) => body
            .split(|character: char| !character.is_ascii_digit())
            .filter_map(|token| token.parse::<u16>().ok())
            .any(is_server_status),
    }
}

fn status_field(json: &serde_json::Value) -> Option<u16> {
    let status = json.get("status").or_else(|| json.get("code"))?.as_u64()?;
    u16::try_from(status).ok()
}

fn is_server_status(code: u16) -> bool {
    (500..=599).contains(&code)
}

/// Map the pre-flight `auth()` failure: a refusal speaks in the registry's words, anything else reads as an unreachable registry.
pub(crate) fn auth_error(
    reference: &Reference,
    auth: &RegistryAuth,
    err: OciDistributionError,
) -> anyhow::Error {
    match token_refusal(&err) {
        Some(refused_because) => refused(reference, auth, err, refused_because),
        None => anyhow::Error::new(err).context(format!(
            "could not reach {} to authenticate the push",
            reference.registry()
        )),
    }
}

/// A 5xx on the token handshake is an outage of the authorization itself, so it never earns the recipe.
fn token_refusal(err: &OciDistributionError) -> Option<Refusal> {
    match err {
        OciDistributionError::ServerError {
            code, message, url, ..
        } if is_server_status(*code) => Some(Refusal::Unavailable(
            registry_words(message)
                .unwrap_or_else(|| format!("{url} answered HTTP {code} with no explanation")),
        )),
        _ => refusal(err),
    }
}

/// Map an upload-phase (`push_blob` / `push_manifest_raw`) failure: a refusal speaks in the registry's words, anything else surfaces verbatim under `context`.
pub(crate) fn push_error(
    reference: &Reference,
    auth: &RegistryAuth,
    err: OciDistributionError,
    context: String,
) -> anyhow::Error {
    match refusal(&err) {
        Some(refused_because) => refused(reference, auth, err, refused_because),
        None => anyhow::Error::new(err).context(context),
    }
}

fn refused(
    reference: &Reference,
    auth: &RegistryAuth,
    err: impl std::fmt::Display,
    refusal: Refusal,
) -> anyhow::Error {
    crate::log::debug!(registry = %reference.registry(), error = %err, "registry refused the push auth");
    let registry = reference.registry();
    match refusal {
        Refusal::LoginCanFix(words) => login_refused_message(reference, auth, words),
        Refusal::Denied(words) => anyhow::anyhow!(
            "{registry} refused the push of {}\n\n{words}",
            reference.repository()
        ),
        Refusal::Unavailable(words) => {
            anyhow::anyhow!("{registry} cannot authorize the push right now\n\n{words}")
        }
    }
}

fn login_refused_message(
    reference: &Reference,
    auth: &RegistryAuth,
    words: Option<String>,
) -> anyhow::Error {
    let registry = reference.registry();
    let recipe = login_recipe(registry);
    let said = match words {
        Some(words) => format!("\n\n{words}"),
        None => String::new(),
    };
    match auth {
        RegistryAuth::Anonymous => anyhow::anyhow!(
            "{registry} denied the push — no login is stored for it{said}\n\n\
             Sign in with a token that has push access, then push again:\n\n{recipe}"
        ),
        _ => anyhow::anyhow!(
            "{registry} denied the push — it refused the stored login{said}\n\n\
             The token may be expired, missing push scope, or not allowed to\n\
             write {}. Refresh the login, then push again:\n\n{recipe}",
            reference.repository()
        ),
    }
}

fn login_recipe(registry: &str) -> String {
    if registry == "ghcr.io" {
        return "\x20   gh auth refresh --scopes write:packages\n\
                \x20   gh auth token | lns login ghcr.io --username <YOUR-GITHUB-USER> --password-stdin\n\n\
                (or paste any GitHub token with the write:packages scope via --password-stdin)"
            .into();
    }
    format!("\x20   echo <TOKEN> | lns login {registry} --username <USER> --password-stdin")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::registry_auth::{RegistryAuthFile, RegistryCredential};
    use oci_client::errors::OciError;
    use std::io;

    fn reference() -> Reference {
        "ghcr.io/acme/my-sandbox:1.0.0".parse().unwrap()
    }

    fn hub_reference() -> Reference {
        "hub.lns.run/acme/hermes:1.0.0".parse().unwrap()
    }

    fn envelope(code: OciErrorCode, message: &str) -> OciEnvelope {
        OciEnvelope {
            errors: vec![OciError {
                code,
                message: message.into(),
                detail: serde_json::Value::Null,
            }],
        }
    }

    #[test]
    fn a_stored_login_for_the_target_registry_arms_basic_auth() {
        let mut file = RegistryAuthFile::new();
        file.insert(
            "ghcr.io".into(),
            RegistryCredential {
                username: "octocat".into(),
                secret: "ghp_token".into(),
            },
        );

        let auth = select_auth(Ok(file), "ghcr.io");

        assert!(
            matches!(auth, RegistryAuth::Basic(user, secret) if user == "octocat" && secret == "ghp_token"),
            "a stored credential for the target registry must arm basic auth"
        );
    }

    #[test]
    fn a_registry_with_no_stored_login_pushes_anonymously() {
        let auth = select_auth(Ok(RegistryAuthFile::new()), "ghcr.io");
        assert!(matches!(auth, RegistryAuth::Anonymous));
    }

    #[test]
    fn an_unreadable_auth_store_falls_back_to_anonymous() {
        let loaded = Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "corrupt auth store",
        ));

        let auth = select_auth(loaded, "ghcr.io");

        assert!(
            matches!(auth, RegistryAuth::Anonymous),
            "a corrupt store must not abort the push; it falls back to anonymous auth"
        );
    }

    #[test]
    fn an_anonymous_401_points_at_lns_login() {
        let err = auth_error(
            &reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::UnauthorizedError {
                url: "https://ghcr.io/v2/".into(),
            },
        );
        let text = format!("{err:#}");
        assert!(text.contains("ghcr.io denied the push"), "{text}");
        assert!(text.contains("no login is stored"), "{text}");
    }

    #[test]
    fn a_ghcr_refusal_teaches_the_gh_cli_token_recipe() {
        let err = auth_error(
            &reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::UnauthorizedError {
                url: "https://ghcr.io/v2/".into(),
            },
        );
        let text = format!("{err:#}");
        assert!(
            text.contains("gh auth refresh --scopes write:packages"),
            "{text}"
        );
        assert!(text.contains("gh auth token | lns login ghcr.io"), "{text}");
    }

    #[test]
    fn a_generic_registry_refusal_gets_the_plain_login_recipe() {
        let reference: Reference = "registry.example.test/team/thing:1".parse().unwrap();
        let err = auth_error(
            &reference,
            &RegistryAuth::Anonymous,
            OciDistributionError::UnauthorizedError {
                url: "https://registry.example.test/v2/".into(),
            },
        );
        let text = format!("{err:#}");
        assert!(
            text.contains(
                "echo <TOKEN> | lns login registry.example.test --username <USER> --password-stdin"
            ),
            "{text}"
        );
        assert!(!text.contains("gh auth"), "{text}");
    }

    #[test]
    fn a_transport_failure_surfaces_the_error_without_the_login_recipe() {
        let err = auth_error(
            &reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::GenericError(Some("dns error: failed to lookup ghcr.io".into())),
        );
        let text = format!("{err:#}");
        assert!(
            !text.contains("no login is stored"),
            "a connectivity failure must not be misreported as a missing login: {text}"
        );
        assert!(
            text.contains("could not reach ghcr.io") && text.contains("dns error"),
            "the real connectivity error must surface: {text}"
        );
    }

    #[test]
    fn a_stored_credential_401_names_scope_and_repository() {
        let auth = RegistryAuth::Basic("user".into(), "secret".into());
        let err = auth_error(
            &reference(),
            &auth,
            OciDistributionError::UnauthorizedError {
                url: "https://ghcr.io/v2/".into(),
            },
        );
        let text = format!("{err:#}");
        assert!(text.contains("it refused the stored login"), "{text}");
        assert!(text.contains("acme/my-sandbox"), "{text}");
        assert!(
            !text.contains("secret"),
            "the secret must never surface: {text}"
        );
    }

    #[test]
    fn an_empty_token_refusal_still_teaches_the_login_recipe() {
        let err = auth_error(
            &reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::AuthenticationFailure(String::new()),
        );
        let text = format!("{err:#}");
        assert!(
            text.contains("no login is stored") && text.contains("lns login ghcr.io"),
            "a bodyless token refusal is all lns knows, so it keeps the recipe: {text}"
        );
    }

    #[test]
    fn a_token_refusal_shows_the_registrys_own_words_instead_of_the_recipe() {
        let err = auth_error(
            &hub_reference(),
            &RegistryAuth::Basic("user".into(), "secret".into()),
            OciDistributionError::AuthenticationFailure(
                r#"{"details":"the name acme belongs to another namespace; your organization is acme-2 on this hub, push to acme-2/hermes"}"#
                    .into(),
            ),
        );
        let text = format!("{err:#}");
        assert!(
            text.contains("your organization is acme-2 on this hub, push to acme-2/hermes"),
            "the registry explained the refusal; that explanation is the message: {text}"
        );
        assert!(
            !text.contains("lns login"),
            "logging in again does not fix a name collision: {text}"
        );
        assert!(
            !text.contains("details"),
            "the JSON envelope is not part of the registry's words: {text}"
        );
    }

    #[test]
    fn a_token_5xx_says_the_registry_cannot_authorize_right_now() {
        let err = auth_error(
            &hub_reference(),
            &RegistryAuth::Basic("user".into(), "secret".into()),
            OciDistributionError::ServerError {
                code: 503,
                url: "https://hub.lns.run/token".into(),
                message: r#"{"details":"the hub cannot confirm your Lens Business ID membership right now"}"#.into(),
            },
        );
        let text = format!("{err:#}");
        assert!(
            text.contains("cannot authorize the push right now"),
            "a 5xx from /token is an outage, not a credential refusal: {text}"
        );
        assert!(
            text.contains("the hub cannot confirm your Lens Business ID membership right now"),
            "{text}"
        );
        assert!(
            !text.contains("lns login"),
            "a login cannot fix an outage: {text}"
        );
    }

    #[test]
    fn a_bodyless_token_5xx_still_reports_the_status() {
        let err = auth_error(
            &hub_reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::ServerError {
                code: 502,
                url: "https://hub.lns.run/token".into(),
                message: String::new(),
            },
        );
        let text = format!("{err:#}");
        assert!(
            text.contains("cannot authorize the push right now") && text.contains("502"),
            "{text}"
        );
    }

    #[test]
    fn a_token_body_that_reports_a_5xx_reads_as_an_outage() {
        let err = auth_error(
            &hub_reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::AuthenticationFailure(
                "503 Service Temporarily Unavailable".into(),
            ),
        );
        let text = format!("{err:#}");
        assert!(
            text.contains("cannot authorize the push right now")
                && text.contains("503 Service Temporarily Unavailable"),
            "{text}"
        );
        assert!(!text.contains("lns login"), "{text}");
    }

    #[test]
    fn a_json_status_field_of_503_reads_as_an_outage() {
        let err = auth_error(
            &hub_reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::AuthenticationFailure(
                r#"{"status":503,"message":"the upstream is down"}"#.into(),
            ),
        );
        let text = format!("{err:#}");
        assert!(
            text.contains("cannot authorize the push right now")
                && text.contains("the upstream is down"),
            "{text}"
        );
    }

    #[test]
    fn a_number_inside_a_json_message_is_not_a_status() {
        let err = auth_error(
            &hub_reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::AuthenticationFailure(
                r#"{"details":"the manifest is over the 500 KiB cap"}"#.into(),
            ),
        );
        let text = format!("{err:#}");
        assert!(
            text.contains("refused the push") && text.contains("over the 500 KiB cap"),
            "a number in prose does not make a refusal an outage: {text}"
        );
    }

    #[test]
    fn an_unknown_token_body_surfaces_verbatim_but_trimmed() {
        let err = auth_error(
            &hub_reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::AuthenticationFailure("  push a signed artifact  \n".into()),
        );
        let text = format!("{err:#}");
        assert!(text.contains("hub.lns.run refused the push"), "{text}");
        assert!(
            text.contains("\n\npush a signed artifact"),
            "an unparseable body is still the registry's words: {text}"
        );
    }

    #[test]
    fn a_json_body_with_no_message_field_surfaces_verbatim() {
        let err = auth_error(
            &hub_reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::AuthenticationFailure(r#"{"errors":[]}"#.into()),
        );
        let text = format!("{err:#}");
        assert!(text.contains(r#"{"errors":[]}"#), "{text}");
    }

    #[test]
    fn a_token_body_that_is_an_oci_envelope_renders_as_code_and_message() {
        let err = auth_error(
            &hub_reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::AuthenticationFailure(
                r#"{"errors":[{"code":"DENIED","message":"push to acme-2/hermes"}]}"#.into(),
            ),
        );
        let text = format!("{err:#}");
        assert!(text.contains("DENIED: push to acme-2/hermes"), "{text}");
    }

    #[test]
    fn a_403_envelope_at_upload_renders_as_code_and_message_lines() {
        let err = push_error(
            &hub_reference(),
            &RegistryAuth::Basic("user".into(), "secret".into()),
            OciDistributionError::RegistryError {
                envelope: envelope(
                    OciErrorCode::Denied,
                    "the name acme belongs to another namespace; push to acme-2/hermes",
                ),
                url: "https://hub.lns.run/v2/acme/hermes/blobs/uploads/".into(),
            },
            "pushing blob sha256:abc".into(),
        );
        let text = format!("{err:#}");
        assert!(
            text.contains(
                "DENIED: the name acme belongs to another namespace; push to acme-2/hermes"
            ),
            "an envelope renders as code: message lines: {text}"
        );
        assert!(
            !text.contains("envelope:") && !text.contains("OCI API error"),
            "the Debug-ish oci-client form must not reach the user: {text}"
        );
        assert!(
            !text.contains("lns login"),
            "an authenticated-but-not-allowed refusal is not a login problem: {text}"
        );
    }

    #[test]
    fn a_multi_word_envelope_code_renders_screaming_snake_case() {
        let err = push_error(
            &hub_reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::RegistryError {
                envelope: envelope(OciErrorCode::NameUnknown, "no such repository"),
                url: "https://hub.lns.run/v2/acme/hermes/blobs/uploads/".into(),
            },
            "pushing blob sha256:abc".into(),
        );
        assert!(
            format!("{err:#}").contains("NAME_UNKNOWN: no such repository"),
            "{err:#}"
        );
    }

    #[test]
    fn a_messageless_envelope_error_renders_its_code_alone() {
        let err = push_error(
            &hub_reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::RegistryError {
                envelope: envelope(OciErrorCode::Denied, ""),
                url: "https://hub.lns.run/v2/acme/hermes/blobs/uploads/".into(),
            },
            "pushing blob sha256:abc".into(),
        );
        let text = format!("{err:#}");
        assert!(text.contains("DENIED"), "{text}");
        assert!(!text.contains("DENIED:"), "{text}");
    }

    #[test]
    fn an_empty_envelope_surfaces_under_its_push_context() {
        let err = push_error(
            &hub_reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::RegistryError {
                envelope: OciEnvelope { errors: vec![] },
                url: "https://hub.lns.run/v2/acme/hermes/blobs/uploads/".into(),
            },
            "pushing blob sha256:abc".into(),
        );
        let text = format!("{err:#}");
        assert!(
            text.contains("pushing blob sha256:abc"),
            "an envelope with nothing in it has no words to show: {text}"
        );
    }

    #[test]
    fn a_403_with_a_message_at_upload_is_a_refusal_not_a_login_problem() {
        let auth = RegistryAuth::Basic("octocat".into(), "ghp_token".into());
        let err = push_error(
            &reference(),
            &auth,
            OciDistributionError::ServerError {
                code: 403,
                url: "https://ghcr.io/v2/acme/my-sandbox/blobs/uploads/".into(),
                message: "installation not allowed to write package".into(),
            },
            "pushing blob sha256:abc".into(),
        );
        let text = format!("{err:#}");
        assert!(text.contains("ghcr.io refused the push"), "{text}");
        assert!(
            text.contains("installation not allowed to write package"),
            "{text}"
        );
        assert!(
            !text.contains("lns login"),
            "a 403 that explains itself is not a stale-login problem: {text}"
        );
        assert!(
            !text.contains("pushing blob"),
            "the upload-phase context must not mask the registry's words: {text}"
        );
    }

    #[test]
    fn a_bodyless_403_at_upload_earns_the_push_scope_recipe() {
        let auth = RegistryAuth::Basic("octocat".into(), "ghp_token".into());
        let err = push_error(
            &reference(),
            &auth,
            OciDistributionError::ServerError {
                code: 403,
                url: "https://ghcr.io/v2/acme/my-sandbox/blobs/uploads/".into(),
                message: String::new(),
            },
            "pushing blob sha256:abc".into(),
        );
        let text = format!("{err:#}");
        assert!(
            text.contains("missing push scope") && text.contains("gh auth token | lns login"),
            "with no explanation, a 403 is still best read as a scope problem: {text}"
        );
    }

    #[test]
    fn a_401_at_upload_shows_the_registrys_words_before_the_recipe() {
        let err = push_error(
            &reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::ServerError {
                code: 401,
                url: "https://ghcr.io/v2/".into(),
                message: "authentication required".into(),
            },
            "pushing manifest to ghcr.io/acme/my-sandbox:1.0.0".into(),
        );
        let text = format!("{err:#}");
        let said = text
            .find("authentication required")
            .expect("the registry's words must surface");
        let recipe = text.find("lns login").expect("a 401 keeps the recipe");
        assert!(said < recipe, "the registry speaks first: {text}");
        assert!(text.contains("no login is stored"), "{text}");
    }

    #[test]
    fn a_non_auth_upload_failure_surfaces_verbatim_under_its_context() {
        let err = push_error(
            &reference(),
            &RegistryAuth::Basic("u".into(), "p".into()),
            OciDistributionError::ServerError {
                code: 500,
                url: "https://ghcr.io/v2/acme/my-sandbox/blobs/uploads/".into(),
                message: "internal".into(),
            },
            "pushing blob sha256:abc".into(),
        );
        let text = format!("{err:#}");
        assert!(
            text.contains("pushing blob sha256:abc"),
            "a 5xx at upload keeps its push-phase context: {text}"
        );
        assert!(
            !text.contains("denied the push"),
            "a server error must not be miscast as a credential refusal: {text}"
        );
        assert!(
            text.contains("code: 500"),
            "the real server error must surface: {text}"
        );
    }

    #[test]
    fn a_preflight_403_shows_the_registrys_words() {
        let err = auth_error(
            &reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::ServerError {
                code: 403,
                url: "https://ghcr.io/token".into(),
                message: "denied: no write access".into(),
            },
        );
        let text = format!("{err:#}");
        assert!(text.contains("denied: no write access"), "{text}");
        assert!(!text.contains("lns login"), "{text}");
    }
}
