use lns_artifact::tools::registry;

use super::{Libc, ProvisionError, ProvisionTarget, ToolRef};

pub use registry::{backend_for, is_supported_backend, source_host};

/// Whether the tool set can be provisioned at all. The authoring verbs run the same gate against the same snapshot, so reaching this on a launch means the definition was published by an older build or hand-edited.
pub fn refuse_unknown_tools(requests: &[ToolRef]) -> Result<(), ProvisionError> {
    registry::refuse_unprovisionable(requests).map_err(ProvisionError::Unprovisionable)
}

/// The libc gate can only run here: it needs the base image's layers, which the authoring verbs never fetch.
pub fn refuse_libc_unsupported(
    requests: &[ToolRef],
    target: &ProvisionTarget,
    image: &str,
) -> Result<(), ProvisionError> {
    if target.libc != Libc::Musl {
        return Ok(());
    }
    for tool in requests {
        if let Some((_, reason)) = registry::MUSL_UNSUPPORTED
            .iter()
            .find(|(name, _)| *name == tool.name)
        {
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
    libc == Libc::Musl && registry::MUSL_COMPANION_TOOLS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Arch;

    fn tool(spec: &str) -> ToolRef {
        lns_artifact::tools::parse(spec).expect("valid tool spec")
    }

    fn target(libc: Libc) -> ProvisionTarget {
        ProvisionTarget {
            arch: Arch::Aarch64,
            libc,
        }
    }

    #[test]
    fn an_unprovisionable_tool_refuses_the_launch_with_the_shared_wording() {
        let err = refuse_unknown_tools(&[tool("definitely-not-a-tool@1")]).unwrap_err();
        assert!(
            err.to_string().contains("not a tool lns can provision"),
            "got: {err}"
        );
        refuse_unknown_tools(&[tool("node@22"), tool("jq@latest")]).unwrap();
    }

    #[test]
    fn deno_on_a_musl_image_is_refused_with_both_remedies() {
        let err = refuse_libc_unsupported(&[tool("deno@2")], &target(Libc::Musl), "alpine:3.20")
            .unwrap_err();
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
        refuse_libc_unsupported(&[tool("deno@2")], &target(Libc::Gnu), "debian:12-slim").unwrap();
    }

    #[test]
    fn a_musl_tool_without_a_known_gap_passes_the_libc_gate() {
        refuse_libc_unsupported(&[tool("node@22")], &target(Libc::Musl), "alpine:3.20").unwrap();
    }

    #[test]
    fn musl_companions_apply_to_node_and_bun_only_on_musl() {
        assert!(needs_musl_companions("node", Libc::Musl));
        assert!(needs_musl_companions("bun", Libc::Musl));
        assert!(!needs_musl_companions("node", Libc::Gnu));
        assert!(!needs_musl_companions("python", Libc::Musl));
    }
}
