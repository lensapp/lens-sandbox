use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use lns_policy::credentials::CredentialStateFile;
use lns_policy::pulled::{PulledCatalog, default_pulled_connectors_path};
use oci_client::Reference;
use oci_client::manifest::OciImageManifest;

use super::{ManifestSource, PullConnectorOutcome};
use crate::image::Registry;

impl<R: Registry> ManifestSource for R {
    fn pull_manifest_and_config(
        &self,
        reference: &Reference,
    ) -> impl std::future::Future<Output = Result<(OciImageManifest, String, String)>> + Send {
        Registry::pull_manifest_and_config(self, reference)
    }
}

/// `lns connector pull <ref>`: pull the connector artifact, gate a replacement of an already-consented connector, and persist the pulled catalog when it lands.
pub async fn pull(reference: &str, confirm_replace: bool) -> Result<PullConnectorOutcome> {
    let client = crate::image::caching_registry_for(reference)?;
    let path = default_pulled_connectors_path();
    let mut pulled = PulledCatalog::load_or_default(&path)?;
    let credentials = load_credentials();
    let now = crate::time_fmt::rfc3339_from_unix(now_unix_secs());
    let outcome = super::pull_connector_with(
        &client,
        reference,
        &now,
        &mut pulled,
        &credentials,
        confirm_replace,
    )
    .await?;
    if matches!(outcome, PullConnectorOutcome::Pulled { .. }) {
        pulled.save_atomic(&path)?;
    }
    Ok(outcome)
}

fn load_credentials() -> CredentialStateFile {
    use crate::credential_flow::store::{
        CredentialStore, JsonFileCredentialStore, default_credentials_path,
    };
    JsonFileCredentialStore::new(default_credentials_path())
        .load()
        .unwrap_or_default()
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
