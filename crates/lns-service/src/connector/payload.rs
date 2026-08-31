//! What a granted method supplies to the running guest, per `docs/sandbox-spec.md` §3.2.4.

use lns_artifact::connector::Method;
use lns_ipc::SecretValues;
use lns_spec::{Credential, InjectionKind};

use crate::approval_flow::protocol::{GrantedPayload, WireCredential, WireFile, WireInjection};

/// Everything a granted method contributes: its egress, the credentials the boundary arms, its plain `env`, and the files it writes.
pub fn granted_payload(method: &Method, values: &SecretValues) -> GrantedPayload {
    GrantedPayload {
        egress: policy_of(method),
        credentials: method
            .credentials
            .iter()
            .map(|credential| armed(credential, values))
            .collect(),
        env: method.env.clone(),
        files: inline_files(method),
    }
}

/// Every inline file, at the path its entry names. A `path` fileset supplies nothing yet — install keeps its layer, but nothing expands it here, so the method carrying one is not offerable either.
fn inline_files(method: &Method) -> Vec<WireFile> {
    let mut files = Vec::new();
    for fileset in &method.filesets {
        for (name, content) in fileset.inline.iter().flatten() {
            let path = lns_artifact::connector::guest_file(&fileset.guest_path, name);
            files.push(WireFile::text(&path, content, None).owned_by(fileset.owner));
        }
    }
    files
}

fn policy_of(method: &Method) -> lns_policy::Policy {
    lns_policy::Policy {
        network: lns_policy::NetworkPolicy {
            egress: method.egress.clone(),
        },
        ..lns_policy::Policy::default()
    }
}

/// One credential as the boundary receives it. A value this machine does not hold leaves every injection **unarmed** — the domain is declared so the placeholder's first use is gated, but no secret is substituted (§3.2.4).
fn armed(credential: &Credential, values: &SecretValues) -> WireCredential {
    let value = supplied(credential, values);
    WireCredential {
        id: credential.owner().to_string(),
        env_var: credential.env_var.clone(),
        placeholder: Some(credential.placeholder.clone()),
        injections: credential
            .injections
            .iter()
            .map(|injection| one_injection(injection, value.as_deref()))
            .collect(),
    }
}

/// The value behind a credential: the `field` its method's `auth` produced, or the one value a single-credential method collected.
fn supplied(credential: &Credential, values: &SecretValues) -> Option<String> {
    let key = credential.field.as_deref().unwrap_or(credential.owner());
    values.0.get(key).cloned()
}

fn one_injection(injection: &lns_spec::InjectionDef, value: Option<&str>) -> WireInjection {
    let domain = injection.domain.clone();
    match injection.kind {
        InjectionKind::UriPlaceholder => WireInjection::UriPlaceholder {
            domain,
            value: value.unwrap_or_default().to_string(),
        },
        InjectionKind::ApiKeyHeader => WireInjection::Header {
            domain,
            header: injection.header.clone().unwrap_or_default(),
            value: value.unwrap_or_default().to_string(),
        },
        kind => WireInjection::Header {
            domain,
            header: AUTHORIZATION.to_string(),
            value: value
                .map(|value| authorization(kind, value))
                .unwrap_or_default(),
        },
    }
}

const AUTHORIZATION: &str = "Authorization";

fn authorization(kind: InjectionKind, value: &str) -> String {
    match kind {
        InjectionKind::TokenHeader => format!("token {value}"),
        InjectionKind::BasicXAccessToken => format!(
            "Basic {}",
            crate::base64::encode(format!("x-access-token:{value}").as_bytes())
        ),
        _ => format!("Bearer {value}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn method(json: serde_json::Value) -> Method {
        let document = serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": "some-provider",
            "spec": { "serves": ["api.some-provider.example"], "methods": [json] },
        });
        lns_artifact::connector::parse(document.to_string().as_bytes())
            .expect("a valid connector")
            .spec
            .methods
            .remove(0)
    }

    #[test]
    fn an_inline_fileset_is_supplied_under_the_path_the_method_names() {
        let granted = granted_payload(
            &method(serde_json::json!({
                "name": "token",
                "auth": { "kind": "token" },
                "filesets": [{
                    "guestPath": "~/.some-provider",
                    "inline": { "config.json": "{}", "nested/hint.txt": "read me" },
                }],
            })),
            &SecretValues::default(),
        );
        assert_eq!(
            granted
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            [
                "~/.some-provider/config.json",
                "~/.some-provider/nested/hint.txt"
            ],
            "an inline key is a path under the directory the entry names, so the guest writes the tree the author wrote"
        );
        assert_eq!(
            granted.files[0].content,
            crate::approval_flow::protocol::WireFileContent::Content("{}".to_string()),
            "inline content is text by its own shape, so base64 would only obscure it"
        );
        assert_eq!(
            granted.files[0].mode, None,
            "core writes 0600, which is what a credentials file wants and what the spec states"
        );
    }

    #[test]
    fn one_guest_file_has_one_spelling_however_the_document_wrote_it() {
        let granted = granted_payload(
            &method(serde_json::json!({
                "name": "token",
                "auth": { "kind": "token" },
                "filesets": [{ "guestPath": "~/.a//b/", "inline": { "c.json": "{}" } }],
            })),
            &SecretValues::default(),
        );
        assert_eq!(
            granted.files[0].path, "~/.a/b/c.json",
            "an empty segment is legal in a guestPath, so two spellings of one file would slip past the rule refusing two connectors that write it"
        );
    }

    #[test]
    fn a_fileset_the_document_pins_to_root_says_so_on_the_wire() {
        let granted = granted_payload(
            &method(serde_json::json!({
                "name": "token",
                "auth": { "kind": "token" },
                "filesets": [
                    { "guestPath": "~/.a", "inline": { "a.txt": "a" }, "owner": "root" },
                    { "guestPath": "~/.b", "inline": { "b.txt": "b" } },
                ],
            })),
            &SecretValues::default(),
        );
        assert_eq!(
            granted.files[0].owner,
            Some(crate::approval_flow::protocol::WireFileOwner::Root),
            "owner: root is how a document keeps a file from the workload, so dropping it would hand over what it withheld"
        );
        assert_eq!(
            granted.files[1].owner, None,
            "the workload is core's own default, so naming it would only add a field to every frame"
        );
    }

    fn token_method(kind: &str) -> Method {
        method(serde_json::json!({
            "name": "token",
            "auth": { "kind": "token" },
            "credentials": [{
                "envVar": "SOME_TOKEN",
                "placeholder": "some-provider-LNSPLACEHOLDER00",
                "injections": [{ "kind": kind, "domain": "api.some-provider.example" }],
            }],
        }))
    }

    fn values(pairs: &[(&str, &str)]) -> SecretValues {
        SecretValues(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    fn injection_of(payload: &GrantedPayload) -> &WireInjection {
        &payload.credentials[0].injections[0]
    }

    fn header(header: &str, value: &str) -> WireInjection {
        WireInjection::Header {
            domain: "api.some-provider.example".to_string(),
            header: header.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn a_bearer_credential_carries_the_value_the_machine_holds() {
        let payload = granted_payload(
            &token_method("bearer_header"),
            &values(&[("SOME_TOKEN", "sk-live")]),
        );
        assert_eq!(
            *injection_of(&payload),
            header("Authorization", "Bearer sk-live")
        );
        assert_eq!(
            payload.credentials[0].placeholder.as_deref(),
            Some("some-provider-LNSPLACEHOLDER00"),
            "the workload sees the placeholder; only the boundary sees the value"
        );
    }

    #[test]
    fn each_scheme_writes_the_header_its_service_expects() {
        for (kind, expected) in [
            ("bearer_header", "Bearer sk-live"),
            ("token_header", "token sk-live"),
        ] {
            let payload =
                granted_payload(&token_method(kind), &values(&[("SOME_TOKEN", "sk-live")]));
            assert_eq!(
                *injection_of(&payload),
                header("Authorization", expected),
                "{kind}"
            );
        }
    }

    #[test]
    fn a_basic_scheme_sends_the_token_as_the_password_of_a_fixed_user() {
        let payload = granted_payload(
            &token_method("basic_x_access_token"),
            &values(&[("SOME_TOKEN", "sk-live")]),
        );
        assert_eq!(
            *injection_of(&payload),
            // base64("x-access-token:sk-live") — the service the scheme exists for reads the token as the password.
            header("Authorization", "Basic eC1hY2Nlc3MtdG9rZW46c2stbGl2ZQ==")
        );
    }

    #[test]
    fn an_api_key_credential_sets_the_header_the_connector_named() {
        let with_header = method(serde_json::json!({
            "name": "token",
            "auth": { "kind": "token" },
            "credentials": [{
                "envVar": "SOME_TOKEN",
                "placeholder": "some-provider-LNSPLACEHOLDER00",
                "injections": [{
                    "kind": "api_key_header",
                    "domain": "api.some-provider.example",
                    "header": "x-api-key",
                }],
            }],
        }));
        let payload = granted_payload(&with_header, &values(&[("SOME_TOKEN", "sk-live")]));
        assert_eq!(
            *injection_of(&payload),
            header("x-api-key", "sk-live"),
            "a service that reads its own header gets that header, not Authorization"
        );
    }

    #[test]
    fn a_uri_credential_is_substituted_in_the_url_rather_than_a_header() {
        let payload = granted_payload(
            &token_method("uri_placeholder"),
            &values(&[("SOME_TOKEN", "sk-live")]),
        );
        assert_eq!(
            *injection_of(&payload),
            WireInjection::UriPlaceholder {
                domain: "api.some-provider.example".to_string(),
                value: "sk-live".to_string(),
            }
        );
    }

    #[test]
    fn no_debug_of_an_injection_can_print_the_value_it_carries() {
        // One `log::debug!` of a policy frame would otherwise put a live credential on the trace stream.
        let payload = granted_payload(
            &token_method("bearer_header"),
            &values(&[("SOME_TOKEN", "sk-live")]),
        );
        let printed = format!("{:?}", injection_of(&payload));
        assert!(!printed.contains("sk-live"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
        assert!(
            printed.contains("api.some-provider.example"),
            "the domain is not a secret and is what makes the line useful: {printed}"
        );
    }

    #[test]
    fn a_uri_credential_is_redacted_too_and_it_is_the_one_that_would_leak_a_whole_url() {
        let payload = granted_payload(
            &token_method("uri_placeholder"),
            &values(&[("SOME_TOKEN", "sk-live")]),
        );
        let printed = format!("{:?}", injection_of(&payload));
        assert!(!printed.contains("sk-live"), "{printed}");
        assert!(printed.starts_with("UriPlaceholder"), "{printed}");
    }

    #[test]
    fn an_unarmed_injection_says_so_rather_than_looking_like_a_redacted_one() {
        let payload = granted_payload(&token_method("bearer_header"), &values(&[]));
        assert!(
            format!("{:?}", injection_of(&payload)).contains("<unarmed>"),
            "the difference between 'holding a value' and 'holding none' is the thing worth seeing in a log"
        );
    }

    #[test]
    fn a_credential_this_machine_holds_no_value_for_ships_unarmed() {
        // §3.2.4: the domain is declared so the placeholder's first use is gated, but no secret is substituted.
        let payload = granted_payload(&token_method("bearer_header"), &values(&[]));
        assert_eq!(
            *injection_of(&payload),
            header("Authorization", ""),
            "an empty value is what tells the boundary this credential is not armed"
        );
    }

    #[test]
    fn a_credential_drawing_on_an_auth_output_reads_that_field() {
        // §4.1: `field` names which of the method's auth outputs supplies the value, so the key is not the variable.
        let drawing = method(serde_json::json!({
            "name": "oauth",
            "auth": { "kind": "oauth_device" },
            "credentials": [{
                "envVar": "SOME_TOKEN",
                "placeholder": "some-provider-LNSPLACEHOLDER00",
                "field": "access_token",
                "injections": [{ "kind": "bearer_header", "domain": "api.some-provider.example" }],
            }],
        }));
        let payload = granted_payload(&drawing, &values(&[("access_token", "sk-from-oauth")]));
        assert_eq!(
            *injection_of(&payload),
            header("Authorization", "Bearer sk-from-oauth")
        );
    }

    #[test]
    fn the_plain_env_a_method_sets_travels_as_it_is_written() {
        let with_env = method(serde_json::json!({
            "name": "open",
            "env": { "SOME_REGION": "eu" },
        }));
        assert_eq!(
            granted_payload(&with_env, &values(&[]))
                .env
                .get("SOME_REGION"),
            Some(&"eu".to_string())
        );
    }

    #[test]
    fn a_granted_method_opens_exactly_the_egress_it_declares() {
        let opening = method(serde_json::json!({
            "name": "open",
            "egress": { "http": [{ "match": "api.some-provider.example", "verdict": "allow" }] },
        }));
        let payload = granted_payload(&opening, &values(&[]));
        assert_eq!(
            payload.egress.network.egress.http[0].match_pattern,
            "api.some-provider.example"
        );
    }
}
