use std::collections::HashMap;
use std::sync::OnceLock;

use super::{Libc, ProvisionError, ProvisionTarget, ToolRef};

/// Regenerated from the pinned mise release's registry by bump-mise; each line is `name<TAB>backend`, preferring the first download-only backend.
const SNAPSHOT: &str = include_str!("registry.snapshot");

const SUPPORTED_BACKEND_KINDS: &[&str] = &["core", "aqua", "ubi", "github", "http", "gitlab"];

const MUSL_UNSUPPORTED: &[(&str, &str)] = &[("deno", "Deno publishes no musl builds")];

/// Tools whose musl builds need the pinned libstdc++/bash companion trees alongside them.
pub const MUSL_COMPANION_TOOLS: &[&str] = &["node", "bun"];

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

pub fn refuse_unknown_tools(requests: &[ToolRef]) -> Result<(), ProvisionError> {
    for tool in requests {
        match backend_for(&tool.name) {
            None => {
                return Err(ProvisionError::Unknown {
                    entry: tool.to_string(),
                    name: tool.name.clone(),
                });
            }
            Some(backend) if !is_supported_backend(backend) => {
                return Err(ProvisionError::UnsupportedBackend {
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

pub fn refuse_libc_unsupported(
    requests: &[ToolRef],
    target: &ProvisionTarget,
    image: &str,
) -> Result<(), ProvisionError> {
    if target.libc != Libc::Musl {
        return Ok(());
    }
    for tool in requests {
        if let Some((_, reason)) = MUSL_UNSUPPORTED.iter().find(|(name, _)| *name == tool.name) {
            return Err(ProvisionError::LibcUnsupported {
                tool: tool.to_string(),
                name: tool.name.clone(),
                image: image.to_string(),
                reason: (*reason).to_string(),
            });
        }
    }
    Ok(())
}

pub fn needs_musl_companions(name: &str, libc: Libc) -> bool {
    libc == Libc::Musl && MUSL_COMPANION_TOOLS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Arch;

    fn tool(spec: &str) -> ToolRef {
        lns_artifact::tools::parse(spec).expect("valid tool spec")
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
        let err = refuse_unknown_tools(&[tool("definitely-not-a-tool@1")]).unwrap_err();
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
        let err = refuse_unknown_tools(&[tool(&format!("{unsupported}@1"))]).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(&unsupported) && msg.contains("bring it via spec.image"),
            "got: {msg}"
        );
    }

    #[test]
    fn known_supported_tools_pass_the_refusal_gate() {
        refuse_unknown_tools(&[tool("node@22"), tool("jq@latest")]).unwrap();
    }

    #[test]
    fn deno_on_a_musl_image_is_refused_with_both_remedies() {
        let target = ProvisionTarget {
            arch: Arch::Aarch64,
            libc: Libc::Musl,
        };
        let err = refuse_libc_unsupported(&[tool("deno@2")], &target, "alpine:3.20").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("deno@2")
                && msg.contains("alpine:3.20")
                && msg.contains("no musl builds")
                && msg.contains("glibc base image")
                && msg.contains("remove deno from spec.tools"),
            "got: {msg}"
        );
    }

    #[test]
    fn deno_on_a_glibc_image_passes_the_libc_gate() {
        let target = ProvisionTarget {
            arch: Arch::Aarch64,
            libc: Libc::Gnu,
        };
        refuse_libc_unsupported(&[tool("deno@2")], &target, "debian:12-slim").unwrap();
    }

    #[test]
    fn musl_companions_apply_to_node_and_bun_only_on_musl() {
        assert!(needs_musl_companions("node", Libc::Musl));
        assert!(needs_musl_companions("bun", Libc::Musl));
        assert!(!needs_musl_companions("node", Libc::Gnu));
        assert!(!needs_musl_companions("python", Libc::Musl));
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
    }
}
