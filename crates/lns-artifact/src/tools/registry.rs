use std::collections::HashMap;
use std::sync::OnceLock;

use super::ToolRef;

/// Regenerated from the pinned mise release's registry by bump-mise; each line is `name<TAB>backend`, preferring the first download-only backend.
const SNAPSHOT: &str = include_str!("registry.snapshot");

/// The download-only backends: a tool installs by fetching a published artifact, with no build step and no second package manager in the guest.
pub const SUPPORTED_BACKEND_KINDS: &[&str] = &["core", "aqua", "ubi", "github", "http", "gitlab"];

/// Tools with no musl build; the launch that knows the image's libc flavor is what refuses them.
pub const MUSL_UNSUPPORTED: &[(&str, &str)] = &[("deno", "Deno publishes no musl builds")];

/// Tools whose musl builds need the pinned libstdc++/bash companion trees alongside them.
pub const MUSL_COMPANION_TOOLS: &[&str] = &["node", "bun"];

/// A declared tool this build cannot provision. Authoring verbs and the launch share it so an author is refused at `validate` and `push` rather than by a consumer's failed launch.
#[derive(Debug, thiserror::Error)]
pub enum ToolRefusal {
    #[error(
        "spec.tools declares \"{entry}\": \"{name}\" is not a tool lns can provision; check the name against mise's registry"
    )]
    Unknown { entry: String, name: String },
    #[error(
        "spec.tools declares \"{entry}\": \"{name}\" needs the {backend} backend, which lns does not provision; bring it via spec.image instead"
    )]
    UnsupportedBackend {
        entry: String,
        name: String,
        backend: String,
    },
}

fn entries() -> &'static HashMap<&'static str, &'static str> {
    static ENTRIES: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        SNAPSHOT
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .collect()
    })
}

pub fn backend_for(name: &str) -> Option<&'static str> {
    entries().get(name).copied()
}

pub fn backend_kind(backend: &str) -> &str {
    backend.split(':').next().unwrap_or(backend)
}

pub fn is_supported_backend(backend: &str) -> bool {
    SUPPORTED_BACKEND_KINDS.contains(&backend_kind(backend))
}

/// The host the tool's bodies are fetched from — audit metadata, derived from the backend shorthand.
pub fn source_host(name: &str, backend: &str) -> String {
    match backend_kind(backend) {
        "core" => core_source_host(name).to_string(),
        "aqua" | "ubi" | "github" => "github.com".to_string(),
        "gitlab" => "gitlab.com".to_string(),
        "http" => backend
            .split_once("://")
            .and_then(|(_, rest)| rest.split('/').next())
            .unwrap_or("upstream")
            .to_string(),
        _ => "upstream".to_string(),
    }
}

fn core_source_host(name: &str) -> &'static str {
    match name {
        "node" => "nodejs.org",
        "python" => "github.com",
        "go" => "go.dev",
        "rust" => "static.rust-lang.org",
        "java" => "api.adoptium.net",
        "deno" => "dl.deno.land",
        "bun" => "github.com",
        "zig" => "ziglang.org",
        "ruby" => "cache.ruby-lang.org",
        _ => "upstream",
    }
}

pub fn refuse_unprovisionable(requests: &[ToolRef]) -> Result<(), ToolRefusal> {
    for tool in requests {
        match backend_for(&tool.name) {
            None => {
                return Err(ToolRefusal::Unknown {
                    entry: tool.to_string(),
                    name: tool.name.clone(),
                });
            }
            Some(backend) if !is_supported_backend(backend) => {
                return Err(ToolRefusal::UnsupportedBackend {
                    entry: tool.to_string(),
                    name: tool.name.clone(),
                    backend: backend_kind(backend).to_string(),
                });
            }
            Some(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(spec: &str) -> ToolRef {
        super::super::parse(spec).expect("valid tool spec")
    }

    #[test]
    fn the_snapshot_maps_the_spike_validated_tools_to_download_backends() {
        for (name, kind) in [
            ("node", "core"),
            ("python", "core"),
            ("go", "core"),
            ("rust", "core"),
            ("bun", "core"),
            ("deno", "core"),
            ("jq", "aqua"),
            ("ripgrep", "aqua"),
            ("terraform", "aqua"),
            ("uv", "aqua"),
            ("gh", "aqua"),
        ] {
            let backend = backend_for(name).unwrap_or_else(|| panic!("{name} not in snapshot"));
            assert_eq!(backend_kind(backend), kind, "for {name}: {backend}");
            assert!(is_supported_backend(backend), "for {name}: {backend}");
        }
    }

    #[test]
    fn an_unknown_tool_is_refused_naming_it() {
        let err = refuse_unprovisionable(&[tool("definitely-not-a-tool@1")]).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("definitely-not-a-tool") && msg.contains("not a tool lns can provision"),
            "got: {msg}"
        );
    }

    #[test]
    fn a_plugin_backed_tool_is_refused_naming_the_backend_and_the_remedy() {
        let unsupported = entries()
            .iter()
            .find(|(_, backend)| !is_supported_backend(backend))
            .map(|(name, _)| (*name).to_string())
            .expect("the snapshot carries at least one unsupported-backend entry");
        let err = refuse_unprovisionable(&[tool(&format!("{unsupported}@1"))]).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(&unsupported) && msg.contains("bring it via spec.image"),
            "got: {msg}"
        );
    }

    #[test]
    fn known_supported_tools_pass_the_refusal_gate() {
        refuse_unprovisionable(&[tool("node@22"), tool("jq@latest")]).unwrap();
    }

    #[test]
    fn source_hosts_derive_from_the_backend_shorthand() {
        assert_eq!(source_host("node", "core:node"), "nodejs.org");
        assert_eq!(source_host("jq", "aqua:jqlang/jq"), "github.com");
        assert_eq!(source_host("some", "gitlab:owner/repo"), "gitlab.com");
        assert_eq!(
            source_host("some", "http:https://dl.example.test/some.tar.gz"),
            "dl.example.test"
        );
        assert_eq!(source_host("some-core", "core:some-core"), "upstream");
        assert_eq!(source_host("some", "npm:some"), "upstream");
    }
}
