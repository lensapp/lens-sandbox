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

pub fn backends() -> impl Iterator<Item = (&'static str, &'static str)> {
    SNAPSHOT.lines().filter_map(|line| line.split_once('\t'))
}

fn entries() -> &'static HashMap<&'static str, &'static str> {
    static ENTRIES: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    ENTRIES.get_or_init(|| backends().collect())
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

/// The host the tool's bodies come from, when the backend reference says so — `None` when it does not, because a guessed host in an audit chain is a false attestation, and this label is never observed from the download itself.
pub fn source_host(name: &str, backend: &str) -> Option<String> {
    let reference = backend.split_once(':').map(|(_, rest)| rest).unwrap_or("");
    match backend_kind(backend) {
        "core" => core_source_host(name).map(str::to_string),
        // An aqua package is `owner/repo` on GitHub unless its first segment is a domain, which is how aqua spells a vendor-hosted release (`aqua:oracle.com/sqlcl`).
        "aqua" => Some(match reference.split('/').next() {
            Some(vendor) if vendor.contains('.') => vendor.to_string(),
            _ => "github.com".to_string(),
        }),
        "ubi" | "github" => Some("github.com".to_string()),
        "gitlab" => Some("gitlab.com".to_string()),
        "http" => reference
            .split_once("://")
            .and_then(|(_, rest)| rest.split('/').next())
            .map(str::to_string),
        _ => None,
    }
}

/// Where each `core:` tool's bytes come from. Hand-maintained and emitted as an audit attestation, so it is re-verified against the engine on every bump (see runbooks/mise-bump.md) — a name that stops being core-backed is caught here by test.
const CORE_SOURCE_HOSTS: &[(&str, &str)] = &[
    ("node", "nodejs.org"),
    ("python", "github.com"),
    ("go", "go.dev"),
    ("rust", "static.rust-lang.org"),
    ("java", "api.adoptium.net"),
    ("deno", "dl.deno.land"),
    ("bun", "github.com"),
    ("zig", "ziglang.org"),
    ("ruby", "cache.ruby-lang.org"),
];

fn core_source_host(name: &str) -> Option<&'static str> {
    CORE_SOURCE_HOSTS
        .iter()
        .find(|(tool, _)| *tool == name)
        .map(|(_, host)| *host)
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
        let unsupported = backends()
            .find(|(_, backend)| !is_supported_backend(backend))
            .map(|(name, _)| name.to_string())
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
    fn a_source_host_is_claimed_only_when_the_backend_reference_names_one() {
        assert_eq!(
            source_host("node", "core:node").as_deref(),
            Some("nodejs.org")
        );
        assert_eq!(
            source_host("jq", "aqua:jqlang/jq").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            source_host("some", "gitlab:owner/repo").as_deref(),
            Some("gitlab.com")
        );
        assert_eq!(
            source_host("some", "http:https://dl.example.test/some.tar.gz").as_deref(),
            Some("dl.example.test")
        );
        // The shipped snapshot carries these shapes, and none of them says where the bytes come from.
        assert_eq!(source_host("elixir", "core:elixir"), None);
        assert_eq!(source_host("dart", "http:dart"), None);
        assert_eq!(source_host("some", "npm:some"), None);
    }

    #[test]
    fn a_vendor_hosted_aqua_package_is_not_attributed_to_github() {
        for (name, backend, host) in [
            ("acli", "aqua:atlassian.com/acli", "atlassian.com"),
            ("dbt-fusion", "aqua:getdbt.com/dbt-fusion", "getdbt.com"),
            ("kiro-cli", "aqua:kiro.dev/kiro-cli", "kiro.dev"),
            ("sqlcl", "aqua:oracle.com/sqlcl", "oracle.com"),
        ] {
            assert_eq!(source_host(name, backend).as_deref(), Some(host));
        }
    }

    #[test]
    fn every_attested_core_host_belongs_to_a_tool_the_snapshot_still_backs_with_core() {
        // The table is an attestation the snapshot diff cannot show. If a bump moves a tool off the core backend, the host we claim for it is no longer the host that serves it.
        for (tool, host) in CORE_SOURCE_HOSTS {
            let backend = backend_for(tool)
                .unwrap_or_else(|| panic!("{tool} claims {host} but left the registry"));
            assert_eq!(
                backend_kind(backend),
                "core",
                "{tool} claims {host} but the snapshot now installs it via {backend}"
            );
        }
    }

    #[test]
    fn every_shipped_entry_either_names_a_host_or_claims_none() {
        // The snapshot is the audit's only provenance input, so this pins that no shape in it produces a guess.
        for (name, backend) in entries().iter() {
            if let Some(host) = source_host(name, backend) {
                assert!(
                    host.contains('.') && !host.contains('/'),
                    "{name} ({backend}) claims {host:?}"
                );
            }
        }
    }
}
