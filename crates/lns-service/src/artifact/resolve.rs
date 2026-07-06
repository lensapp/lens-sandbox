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
    let mut on_path = HashSet::new();
    for component in &bundle.components {
        visit(
            &component.name,
            &component.reference,
            fetcher,
            host_arch,
            &mut on_path,
        )?;
    }
    Ok(())
}

fn visit<F: ComponentFetcher>(
    name: &str,
    reference: &str,
    fetcher: &F,
    host_arch: &str,
    on_path: &mut HashSet<String>,
) -> Result<()> {
    if !on_path.insert(reference.to_string()) {
        bail!("reference cycle in the component graph at {reference}");
    }
    let fetched = match fetcher.fetch(reference) {
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
        && arch != host_arch
    {
        bail!(
            "base image {reference} is built for architecture {arch} but this sandbox runs {host_arch}"
        );
    }
    for child in &fetched.references {
        visit(name, child, fetcher, host_arch, on_path)?;
    }
    on_path.remove(reference);
    Ok(())
}
