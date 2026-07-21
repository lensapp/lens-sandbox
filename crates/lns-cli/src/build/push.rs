use anyhow::{Context, Result};
use http::HeaderValue;
use lns_artifact::build::BuiltArtifact;
use lns_policy::registry_auth::{
    JsonFileRegistryAuthStore, RegistryAuthStore, credential_for, default_registry_auth_path,
};
use oci_client::{
    Reference, RegistryOperation,
    client::{ClientConfig, ClientProtocol},
    secrets::RegistryAuth,
};

/// The stored login for `reference`'s registry, or anonymous when none is recorded.
fn registry_auth_for(reference: &Reference) -> RegistryAuth {
    let store = JsonFileRegistryAuthStore::new(default_registry_auth_path());
    let Ok(file) = store.load() else {
        return RegistryAuth::Anonymous;
    };
    match credential_for(&file, reference.registry()) {
        Some(cred) => RegistryAuth::Basic(cred.username.clone(), cred.secret.clone()),
        None => RegistryAuth::Anonymous,
    }
}

/// Only a genuine credential rejection earns the "sign in" recipe; a DNS/connection/server failure surfaces verbatim so the user fixes connectivity, not authentication.
fn auth_error(
    reference: &Reference,
    auth: &RegistryAuth,
    err: oci_client::errors::OciDistributionError,
) -> anyhow::Error {
    use oci_client::errors::OciDistributionError::{AuthenticationFailure, UnauthorizedError};
    match &err {
        AuthenticationFailure(_) | UnauthorizedError { .. } => auth_refused(reference, auth, err),
        _ => anyhow::Error::new(err).context(format!(
            "could not reach {} to authenticate the push",
            reference.registry()
        )),
    }
}

fn auth_refused(
    reference: &Reference,
    auth: &RegistryAuth,
    err: impl std::fmt::Display,
) -> anyhow::Error {
    crate::log::debug!(registry = %reference.registry(), error = %err, "registry refused the push auth");
    auth_refused_message(reference, auth)
}

fn auth_refused_message(reference: &Reference, auth: &RegistryAuth) -> anyhow::Error {
    let registry = reference.registry();
    let recipe = login_recipe(registry);
    match auth {
        RegistryAuth::Anonymous => anyhow::anyhow!(
            "{registry} denied the push — no login is stored for it\n\n\
             Sign in with a token that has push access, then push again:\n\n{recipe}"
        ),
        _ => anyhow::anyhow!(
            "{registry} denied the push — it refused the stored login\n\n\
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

/// Upload a built artifact's blobs and then its exact manifest bytes to `target`, reusing the stored `lns login` credential (which must carry push scope).
pub(crate) async fn push_artifact(built: &BuiltArtifact, target: &str) -> Result<()> {
    let reference: Reference = target
        .parse()
        .with_context(|| format!("invalid target ref {target}"))?;
    let protocol = if lns_artifact::is_loopback_registry(reference.registry()) {
        ClientProtocol::Http
    } else {
        ClientProtocol::Https
    };
    let client = oci_client::Client::new(ClientConfig {
        protocol,
        ..Default::default()
    });
    let auth = registry_auth_for(&reference);
    client
        .auth(&reference, &auth, RegistryOperation::Push)
        .await
        .map_err(|e| auth_error(&reference, &auth, e))?;
    for blob in &built.blobs {
        client
            .push_blob(&reference, blob.data.clone(), &blob.digest)
            .await
            .with_context(|| format!("pushing blob {}", blob.digest))?;
    }
    let content_type = HeaderValue::from_str(&built.manifest_media_type)
        .context("building manifest content-type header")?;
    client
        .push_manifest_raw(&reference, built.manifest.clone(), content_type)
        .await
        .with_context(|| format!("pushing manifest to {target}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference() -> Reference {
        "ghcr.io/acme/my-sandbox:1.0.0".parse().unwrap()
    }

    #[test]
    fn an_anonymous_refusal_points_at_lns_login_without_the_raw_registry_error() {
        let err = auth_refused(
            &reference(),
            &RegistryAuth::Anonymous,
            r#"{"code":"DENIED"}"#,
        );
        let text = format!("{err:#}");
        assert!(text.contains("ghcr.io denied the push"), "{text}");
        assert!(text.contains("no login is stored"), "{text}");
        assert!(
            !text.contains("DENIED"),
            "the raw registry error belongs on the debug stream only: {text}"
        );
    }

    #[test]
    fn a_ghcr_refusal_teaches_the_gh_cli_token_recipe() {
        let err = auth_refused(&reference(), &RegistryAuth::Anonymous, "denied");
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
        let err = auth_refused(&reference, &RegistryAuth::Anonymous, "denied");
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
    fn an_authentication_failure_gets_the_login_recipe() {
        use oci_client::errors::OciDistributionError;
        let err = auth_error(
            &reference(),
            &RegistryAuth::Anonymous,
            OciDistributionError::UnauthorizedError {
                url: "https://ghcr.io/v2/".into(),
            },
        );
        assert!(
            format!("{err:#}").contains("no login is stored"),
            "a 401 must still point at `lns login`: {err:#}"
        );
    }

    #[test]
    fn a_transport_failure_surfaces_the_error_without_the_login_recipe() {
        use oci_client::errors::OciDistributionError;
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
    fn a_stored_credential_refusal_names_scope_and_repository() {
        let auth = RegistryAuth::Basic("user".into(), "secret".into());
        let err = auth_refused(&reference(), &auth, "DENIED");
        let text = format!("{err:#}");
        assert!(text.contains("it refused the stored login"), "{text}");
        assert!(text.contains("acme/my-sandbox"), "{text}");
        assert!(
            !text.contains("secret"),
            "the secret must never surface: {text}"
        );
    }
}
