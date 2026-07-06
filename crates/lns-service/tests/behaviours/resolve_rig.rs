use lns_service::artifact::resolve::{ComponentFetcher, FetchError, FetchedComponent};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub enum Canned {
    Present(FetchedComponent),
    NeedsLogin { host: String },
}

#[derive(Debug, Default)]
pub struct ResolveRig {
    pub components: Vec<(String, String)>,
    pub canned: HashMap<String, Canned>,
    pub fetched: Vec<String>,
    pub resolved: Option<lns_service::artifact::assembly::ResolvedBundle>,
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
            Some(Canned::Present(component)) => Ok(component.clone()),
            Some(Canned::NeedsLogin { host }) => Err(FetchError::NeedsLogin { host: host.clone() }),
            None => Err(FetchError::NotFound),
        }
    }
}
