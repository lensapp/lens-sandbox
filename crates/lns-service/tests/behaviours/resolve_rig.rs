use lns_service::artifact::resolve::{ComponentFetcher, FetchError, FetchedComponent};
use std::collections::HashMap;
use std::sync::Mutex;

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
    pub fetched: Vec<String>,
    pub error: Option<String>,
    pub ok: bool,
}

pub struct FakeFetcher<'a> {
    pub canned: &'a HashMap<String, Canned>,
    pub calls: Mutex<Vec<String>>,
}

impl<'a> FakeFetcher<'a> {
    pub fn new(canned: &'a HashMap<String, Canned>) -> Self {
        Self {
            canned,
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl ComponentFetcher for FakeFetcher<'_> {
    async fn fetch(&self, reference: &str) -> Result<FetchedComponent, FetchError> {
        self.calls.lock().unwrap().push(reference.to_string());
        match self.canned.get(reference) {
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
