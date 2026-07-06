use crate::artifact::assembly::{ResolvedBundle, ResolvedFileset};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;

pub trait ComponentFetcher {
    fn fetch(
        &self,
        reference: &str,
    ) -> impl Future<Output = std::result::Result<FetchedComponent, FetchError>> + Send;
}

#[derive(Debug, Clone, Default)]
pub struct FetchedComponent {
    pub kind: String,
    pub name: String,
    pub arch: Option<String>,
    pub references: Vec<String>,
    pub base_image: Option<String>,
    pub command: Option<String>,
    pub env: BTreeMap<String, String>,
    pub mount_path: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    DuplicateName(String),
    Cycle(String),
    TooLarge,
    Missing {
        name: String,
        reference: String,
    },
    NeedsLogin {
        host: String,
    },
    UnsupportedKind {
        kind: String,
        reference: String,
    },
    NestedBundle(String),
    ArchMismatch {
        reference: String,
        image_arch: String,
        host_arch: String,
    },
    MissingComponent {
        role: &'static str,
    },
    DuplicateComponent {
        role: &'static str,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::DuplicateName(name) => {
                write!(f, "bundle declares two components named {name}")
            }
            ResolveError::Cycle(reference) => {
                write!(f, "reference cycle in the component graph at {reference}")
            }
            ResolveError::TooLarge => write!(
                f,
                "bundle component graph exceeds {MAX_COMPONENTS} components; refusing to resolve"
            ),
            ResolveError::Missing { name, reference } => {
                write!(
                    f,
                    "component {name} ({reference}) is not present in the registry"
                )
            }
            ResolveError::NeedsLogin { host } => {
                write!(
                    f,
                    "registry host {host} needs a login; run `lns login {host}`"
                )
            }
            ResolveError::UnsupportedKind { kind, reference } => {
                write!(f, "unsupported component kind {kind} in {reference}")
            }
            ResolveError::NestedBundle(reference) => {
                write!(f, "nested bundles are not allowed: {reference}")
            }
            ResolveError::ArchMismatch {
                reference,
                image_arch,
                host_arch,
            } => write!(
                f,
                "base image {reference} is built for architecture {image_arch} but this sandbox runs {host_arch}"
            ),
            ResolveError::MissingComponent { role } => {
                write!(f, "bundle must compose exactly one {role}, but found none")
            }
            ResolveError::DuplicateComponent { role } => {
                write!(
                    f,
                    "bundle must compose exactly one {role}, but found more than one"
                )
            }
        }
    }
}

impl std::error::Error for ResolveError {}

const MAX_COMPONENTS: usize = 256;

struct Walk<'a, F: ComponentFetcher> {
    fetcher: &'a F,
    host_arch: &'a str,
    on_path: HashSet<String>,
    cache: HashMap<String, FetchedComponent>,
    budget: usize,
}

pub async fn resolve<F: ComponentFetcher>(
    bundle: &BundleSpec,
    fetcher: &F,
    host_arch: &str,
) -> Result<ResolvedBundle, ResolveError> {
    let mut names = HashSet::new();
    for component in &bundle.components {
        if !names.insert(component.name.as_str()) {
            return Err(ResolveError::DuplicateName(component.name.clone()));
        }
    }
    let mut walk = Walk {
        fetcher,
        host_arch,
        on_path: HashSet::new(),
        cache: HashMap::new(),
        budget: 0,
    };
    for component in &bundle.components {
        walk.visit(&component.name, &component.reference).await?;
    }
    compose(&bundle.components, &walk.cache)
}

fn compose(
    components: &[DeclaredComponent],
    cache: &HashMap<String, FetchedComponent>,
) -> Result<ResolvedBundle, ResolveError> {
    let mut resolved = ResolvedBundle::default();
    let mut sandboxes = 0;
    let mut agents = 0;
    for component in components {
        if let Some(fetched) = cache.get(&component.reference) {
            match fetched.kind.as_str() {
                "Sandbox" => {
                    sandboxes += 1;
                    resolved.base_image = fetched.base_image.clone().unwrap_or_default();
                }
                "Agent" => {
                    agents += 1;
                    resolved.command = fetched.command.clone();
                    resolved.env = fetched.env.clone();
                }
                "FileSet" => resolved.filesets.push(ResolvedFileset {
                    name: fetched.name.clone(),
                    paths: fetched.mount_path.clone().into_iter().collect(),
                }),
                _ => {}
            }
        }
    }
    if sandboxes == 0 {
        return Err(ResolveError::MissingComponent { role: "sandbox" });
    }
    if sandboxes > 1 {
        return Err(ResolveError::DuplicateComponent { role: "sandbox" });
    }
    if agents == 0 {
        return Err(ResolveError::MissingComponent { role: "agent" });
    }
    if agents > 1 {
        return Err(ResolveError::DuplicateComponent { role: "agent" });
    }
    Ok(resolved)
}

impl<F: ComponentFetcher> Walk<'_, F> {
    async fn visit(&mut self, name: &str, reference: &str) -> Result<(), ResolveError> {
        if self.cache.contains_key(reference) {
            return Ok(());
        }
        if !self.on_path.insert(reference.to_string()) {
            return Err(ResolveError::Cycle(reference.to_string()));
        }
        self.budget += 1;
        if self.budget > MAX_COMPONENTS {
            return Err(ResolveError::TooLarge);
        }
        let fetched = match self.fetcher.fetch(reference).await {
            Ok(fetched) => fetched,
            Err(FetchError::NotFound) => {
                return Err(ResolveError::Missing {
                    name: name.to_string(),
                    reference: reference.to_string(),
                });
            }
            Err(FetchError::NeedsLogin { host }) => return Err(ResolveError::NeedsLogin { host }),
        };
        match fetched.kind.as_str() {
            "Sandbox" | "Agent" | "FileSet" | "Policy" | "Integration" => {}
            "AgentSystem" | "Bundle" => {
                return Err(ResolveError::NestedBundle(reference.to_string()));
            }
            other => {
                return Err(ResolveError::UnsupportedKind {
                    kind: other.to_string(),
                    reference: reference.to_string(),
                });
            }
        }
        if let Some(arch) = &fetched.arch
            && arch != self.host_arch
        {
            return Err(ResolveError::ArchMismatch {
                reference: reference.to_string(),
                image_arch: arch.clone(),
                host_arch: self.host_arch.to_string(),
            });
        }
        for child in &fetched.references {
            Box::pin(self.visit(name, child)).await?;
        }
        self.on_path.remove(reference);
        self.cache.insert(reference.to_string(), fetched);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct InfiniteChain;

    impl ComponentFetcher for InfiniteChain {
        async fn fetch(
            &self,
            reference: &str,
        ) -> std::result::Result<FetchedComponent, FetchError> {
            let n: usize = reference.trim_start_matches('r').parse().unwrap_or(0);
            Ok(FetchedComponent {
                kind: "FileSet".into(),
                references: vec![format!("r{}", n + 1)],
                ..Default::default()
            })
        }
    }

    #[tokio::test]
    async fn an_unbounded_component_chain_is_refused_before_it_exhausts_the_stack() {
        let bundle = BundleSpec {
            components: vec![DeclaredComponent {
                name: "root".into(),
                reference: "r0".into(),
            }],
        };
        let err = resolve(&bundle, &InfiniteChain, "test-arch")
            .await
            .unwrap_err();
        assert_eq!(err, ResolveError::TooLarge);
        assert!(
            err.to_string().contains("exceeds"),
            "an unbounded graph must be refused, got: {err}"
        );
    }
}
