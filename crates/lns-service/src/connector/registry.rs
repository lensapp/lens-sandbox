//! The one connector leaf that reaches a registry.
//!
//! Both arms are thin delegations, so what they delegate to is tested rather
//! than this composition root.

use anyhow::Result;

use super::real::{RealSnapshotFs, read_document};
use super::source::{ConnectorSource, FetchedConnector, Source};

pub(super) struct RegistryConnectors;

impl ConnectorSource for RegistryConnectors {
    async fn fetch(&self, source: &Source) -> Result<FetchedConnector> {
        match source {
            Source::Reference(reference) => {
                let registry = crate::image::caching_registry_for(reference)?;
                crate::image::pull_connector_with(&registry, reference).await
            }
            Source::Local(document) => {
                super::source::read_local(&RealSnapshotFs, read_document, document)
            }
        }
    }
}
