pub mod registry;

pub use lns_artifact::tools::ToolRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Libc {
    Gnu,
    Musl,
}

impl std::fmt::Display for Libc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Libc::Gnu => "gnu",
            Libc::Musl => "musl",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arch {
    Aarch64,
    X86_64,
}

impl std::fmt::Display for Arch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Arch::Aarch64 => "aarch64",
            Arch::X86_64 => "x86_64",
        })
    }
}

pub fn host_arch() -> Arch {
    if cfg!(target_arch = "x86_64") {
        Arch::X86_64
    } else {
        Arch::Aarch64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvisionTarget {
    pub arch: Arch,
    pub libc: Libc,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolCacheKey {
    pub name: String,
    pub resolved: String,
    pub arch: Arch,
    pub libc: Libc,
}

#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
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
    #[error(
        "spec.tools declares {tool} but image {image} is musl-based and {reason}. Use a glibc base image (e.g. debian:12-slim) or remove {name} from spec.tools"
    )]
    LibcUnsupported {
        tool: String,
        name: String,
        image: String,
        reason: String,
    },
    #[error(
        "provisioning {tool} failed: {cause}. Nothing was cached; the next run retries from a clean state"
    )]
    FetchFailed { tool: String, cause: String },
    #[error("tool provisioning infrastructure failed: {0}")]
    Engine(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arch_and_libc_render_as_cache_path_segments() {
        assert_eq!(Arch::Aarch64.to_string(), "aarch64");
        assert_eq!(Arch::X86_64.to_string(), "x86_64");
        assert_eq!(Libc::Gnu.to_string(), "gnu");
        assert_eq!(Libc::Musl.to_string(), "musl");
    }

    #[test]
    fn host_arch_matches_the_compilation_target() {
        let expected = if cfg!(target_arch = "x86_64") {
            Arch::X86_64
        } else {
            Arch::Aarch64
        };
        assert_eq!(host_arch(), expected);
    }

    #[test]
    fn a_fetch_failure_names_the_tool_the_cause_and_the_clean_retry() {
        let err = ProvisionError::FetchFailed {
            tool: "node@22".into(),
            cause: "connection timed out".into(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("node@22")
                && msg.contains("connection timed out")
                && msg.contains("Nothing was cached")
                && msg.contains("retries from a clean state"),
            "got: {msg}"
        );
    }

    #[test]
    fn an_engine_fault_is_distinguished_from_a_tool_fetch_failure() {
        let msg = ProvisionError::Engine("provisioner guest did not boot".into()).to_string();
        assert!(
            msg.contains("infrastructure failed") && msg.contains("did not boot"),
            "got: {msg}"
        );
    }
}
