use lns_service::artifact::resolve::{ComponentFetcher, FetchError, FetchedComponent};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum Canned {
    Present {
        kind: String,
        arch: Option<String>,
        refs: Vec<String>,
    },
    NeedsLogin {
        host: String,
    },
}

#[derive(Debug, Default)]
pub struct ResolveRig {
    pub components: Vec<(String, String)>,
    pub canned: HashMap<String, Canned>,
    pub error: Option<String>,
    pub ok: bool,
}

pub struct FakeFetcher<'a>(pub &'a HashMap<String, Canned>);

impl ComponentFetcher for FakeFetcher<'_> {
    fn fetch(&self, reference: &str) -> Result<FetchedComponent, FetchError> {
        match self.0.get(reference) {
            Some(Canned::Present { kind, arch, refs }) => Ok(FetchedComponent {
                kind: kind.clone(),
                arch: arch.clone(),
                references: refs.clone(),
            }),
            Some(Canned::NeedsLogin { host }) => Err(FetchError::NeedsLogin { host: host.clone() }),
            None => Err(FetchError::NotFound),
        }
    }
}
