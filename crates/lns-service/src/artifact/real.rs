use crate::artifact::fetch::fetch_component;
use crate::artifact::resolve::{ComponentFetcher, FetchError, FetchedComponent};
use crate::image::{RealRegistry, registry_auth_for};

pub struct RealComponentFetcher;

impl ComponentFetcher for RealComponentFetcher {
    async fn fetch(&self, reference: &str) -> Result<FetchedComponent, FetchError> {
        let registry = RealRegistry::with_auth(registry_auth_for(reference));
        fetch_component(&registry, reference).await
    }
}
