use std::collections::BTreeMap;

use anyhow::{Context, Result};
use lns_policy::registry_auth::{
    JsonFileRegistryAuthStore, RegistryAuthStore, credential_for, default_registry_auth_path,
};
use oci_client::{
    Reference,
    client::{ClientConfig, ClientProtocol},
    secrets::RegistryAuth,
};

use super::report;

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

async fn resolve_digest(reference: &str) -> Result<String> {
    let parsed: Reference = reference
        .parse()
        .with_context(|| format!("invalid component ref {reference}"))?;
    let protocol = if lns_artifact::is_loopback_registry(parsed.registry()) {
        ClientProtocol::Http
    } else {
        ClientProtocol::Https
    };
    let client = oci_client::Client::new(ClientConfig {
        protocol,
        ..Default::default()
    });
    let (manifest, digest) = client
        .pull_manifest(&parsed, &registry_auth_for(&parsed))
        .await
        .map_err(|e| anyhow::anyhow!("resolving {reference} to a digest: {e}"))?;
    if matches!(manifest, oci_client::manifest::OciManifest::ImageIndex(_)) {
        anyhow::bail!(
            "component {reference} resolves to a multi-arch index; pin it to a single-platform digest"
        );
    }
    Ok(digest)
}

/// Resolve every floating component ref of a bundle to its current registry digest and pin it, so an authored tag is published as an immutable digest; a non-bundle manifest is returned unchanged.
pub(super) async fn resolve_and_pin(raw: &[u8]) -> Result<Vec<u8>> {
    let json = report::to_json(raw).map_err(|message| anyhow::anyhow!(message))?;
    if !matches!(
        lns_artifact::spec::read_kind(&json),
        Ok(lns_artifact::spec::Kind::AgentSystem)
    ) {
        return Ok(json);
    }
    let refs = lns_artifact::build::unpinned_component_refs(&json)?;
    let mut digests = BTreeMap::new();
    for reference in refs {
        let digest = resolve_digest(&reference).await?;
        digests.insert(reference, digest);
    }
    lns_artifact::build::pin_bundle_components(&json, &digests)
}
