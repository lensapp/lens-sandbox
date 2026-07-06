use anyhow::{Result, bail};
use std::collections::HashSet;

pub trait ComponentFetcher {
    fn fetch(&self, reference: &str) -> std::result::Result<FetchedComponent, FetchError>;
}

#[derive(Debug, Clone)]
pub struct FetchedComponent {
    pub kind: String,
    pub arch: Option<String>,
    pub references: Vec<String>,
}

#[derive(Debug)]
pub enum FetchError {
    NotFound,
    NeedsLogin { host: String },
}

#[derive(Debug, Clone)]
pub struct DeclaredComponent {
    pub name: String,
    pub reference: String,
}

#[derive(Debug, Default, Clone)]
pub struct BundleSpec {
    pub components: Vec<DeclaredComponent>,
}

const MAX_COMPONENTS: usize = 256;

struct Walk<'a, F: ComponentFetcher> {
    fetcher: &'a F,
    host_arch: &'a str,
    on_path: HashSet<String>,
    resolved: HashSet<String>,
    budget: usize,
}

pub fn resolve<F: ComponentFetcher>(
    bundle: &BundleSpec,
    fetcher: &F,
    host_arch: &str,
) -> Result<()> {
    let mut names = HashSet::new();
    for component in &bundle.components {
        if !names.insert(component.name.as_str()) {
            bail!("bundle declares two components named {}", component.name);
        }
    }
    let mut walk = Walk {
        fetcher,
        host_arch,
        on_path: HashSet::new(),
        resolved: HashSet::new(),
        budget: 0,
    };
    for component in &bundle.components {
        walk.visit(&component.name, &component.reference)?;
    }
    Ok(())
}

impl<F: ComponentFetcher> Walk<'_, F> {
    fn visit(&mut self, name: &str, reference: &str) -> Result<()> {
        if self.resolved.contains(reference) {
            return Ok(());
        }
        if !self.on_path.insert(reference.to_string()) {
            bail!("reference cycle in the component graph at {reference}");
        }
        self.budget += 1;
        if self.budget > MAX_COMPONENTS {
            bail!(
                "bundle component graph exceeds {MAX_COMPONENTS} components; refusing to resolve"
            );
        }
        let fetched = match self.fetcher.fetch(reference) {
            Ok(fetched) => fetched,
            Err(FetchError::NotFound) => {
                bail!("component {name} ({reference}) is not present in the registry")
            }
            Err(FetchError::NeedsLogin { host }) => {
                bail!("registry host {host} needs a login; run `lns login {host}`")
            }
        };
        match fetched.kind.as_str() {
            "Sandbox" | "Agent" | "FileSet" | "Policy" | "Integration" => {}
            "AgentSystem" | "Bundle" => bail!("nested bundles are not allowed: {reference}"),
            other => bail!("unsupported component kind {other} in {reference}"),
        }
        if let Some(arch) = &fetched.arch
            && arch != self.host_arch
        {
            bail!(
                "base image {reference} is built for architecture {arch} but this sandbox runs {}",
                self.host_arch
            );
        }
        for child in &fetched.references {
            self.visit(name, child)?;
        }
        self.on_path.remove(reference);
        self.resolved.insert(reference.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct InfiniteChain;

    impl ComponentFetcher for InfiniteChain {
        fn fetch(&self, reference: &str) -> std::result::Result<FetchedComponent, FetchError> {
            let n: usize = reference.trim_start_matches('r').parse().unwrap_or(0);
            Ok(FetchedComponent {
                kind: "FileSet".into(),
                arch: None,
                references: vec![format!("r{}", n + 1)],
            })
        }
    }

    #[test]
    fn an_unbounded_component_chain_is_refused_before_it_exhausts_the_stack() {
        let bundle = BundleSpec {
            components: vec![DeclaredComponent {
                name: "root".into(),
                reference: "r0".into(),
            }],
        };
        let err = resolve(&bundle, &InfiniteChain, "test-arch").unwrap_err();
        assert!(
            format!("{err:#}").contains("exceeds"),
            "an unbounded graph must be refused, got: {err:#}"
        );
    }
}
