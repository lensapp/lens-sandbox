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

fn auth_refused(
    reference: &Reference,
    auth: &RegistryAuth,
    err: impl std::fmt::Display,
) -> anyhow::Error {
    let registry = reference.registry();
    let hint = match auth {
        RegistryAuth::Anonymous => format!(
            "no login is stored for {registry} — sign in with a token that has push access, then push again:\n  lns login {registry} --username <USER> --password-stdin"
        ),
        _ => format!(
            "the stored login for {registry} was refused — the token may be expired, missing push scope, or not allowed to write {}; refresh it:\n  lns login {registry} --username <USER> --password-stdin",
            reference.repository()
        ),
    };
    anyhow::anyhow!("authenticating to {registry}: {err}\n{hint}")
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
        .map_err(|e| auth_refused(&reference, &auth, e))?;
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
    fn an_anonymous_refusal_points_at_lns_login() {
        let err = auth_refused(&reference(), &RegistryAuth::Anonymous, "DENIED");
        let text = format!("{err:#}");
        assert!(text.contains("authenticating to ghcr.io: DENIED"), "{text}");
        assert!(text.contains("no login is stored for ghcr.io"), "{text}");
        assert!(
            text.contains("lns login ghcr.io --username <USER> --password-stdin"),
            "{text}"
        );
    }

    #[test]
    fn a_stored_credential_refusal_names_scope_and_repository() {
        let auth = RegistryAuth::Basic("user".into(), "secret".into());
        let err = auth_refused(&reference(), &auth, "DENIED");
        let text = format!("{err:#}");
        assert!(
            text.contains("the stored login for ghcr.io was refused"),
            "{text}"
        );
        assert!(text.contains("acme/my-sandbox"), "{text}");
        assert!(
            !text.contains("secret"),
            "the secret must never surface: {text}"
        );
    }
}
