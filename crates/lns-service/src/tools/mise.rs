use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

use super::{Arch, Libc};

const MANIFEST: &str = include_str!("../../mise.toml");

const ALPINE_BRANCH: &str = "v3.20";

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub engine: Engine,
    pub provisioner_rootfs: ProvisionerRootfs,
    pub static_curl: StaticCurl,
    #[serde(default)]
    pub companion: Vec<Companion>,
}

#[derive(Debug, Deserialize)]
pub struct Engine {
    pub version: String,
    pub sha256: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct ProvisionerRootfs {
    pub gnu: String,
    pub musl: String,
}

#[derive(Debug, Deserialize)]
pub struct StaticCurl {
    pub version: String,
    pub sha256: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct Companion {
    pub name: String,
    pub version: String,
    pub sha256: BTreeMap<String, String>,
}

pub fn manifest() -> &'static Manifest {
    static PARSED: OnceLock<Manifest> = OnceLock::new();
    PARSED.get_or_init(|| {
        toml::from_str(MANIFEST).expect("mise.toml is checked in and pinned by unit tests")
    })
}

pub fn engine_version() -> &'static str {
    &manifest().engine.version
}

pub fn engine_url(arch: Arch) -> String {
    let version = engine_version();
    format!(
        "https://github.com/jdx/mise/releases/download/v{version}/mise-v{version}-linux-{}-musl",
        mise_release_arch(arch)
    )
}

pub fn engine_sha256(arch: Arch) -> &'static str {
    &manifest().engine.sha256[&arch.to_string()]
}

pub fn rootfs_reference(libc: Libc) -> &'static str {
    match libc {
        Libc::Gnu => &manifest().provisioner_rootfs.gnu,
        Libc::Musl => &manifest().provisioner_rootfs.musl,
    }
}

pub fn curl_url(arch: Arch) -> String {
    format!(
        "https://github.com/moparisthebest/static-curl/releases/download/v{}/curl-{}",
        manifest().static_curl.version,
        release_arch(arch)
    )
}

pub fn curl_sha256(arch: Arch) -> &'static str {
    &manifest().static_curl.sha256[&arch.to_string()]
}

pub fn companion_url(companion: &Companion, arch: Arch) -> String {
    format!(
        "https://dl-cdn.alpinelinux.org/alpine/{ALPINE_BRANCH}/main/{arch}/{}-{}.apk",
        companion.name, companion.version
    )
}

pub fn companion_sha256(companion: &Companion, arch: Arch) -> &str {
    &companion.sha256[&arch.to_string()]
}

fn release_arch(arch: Arch) -> &'static str {
    match arch {
        Arch::Aarch64 => "aarch64",
        Arch::X86_64 => "amd64",
    }
}

fn mise_release_arch(arch: Arch) -> &'static str {
    match arch {
        Arch::Aarch64 => "arm64",
        Arch::X86_64 => "x64",
    }
}

/// The fail-loud engine environment: on any fetch failure mise otherwise silently compiles from source, which cannot work on a bare guest and buries the real error; self-update and the version phone-home never belong in a pinned engine.
pub fn provision_env() -> Vec<(String, String)> {
    [
        ("MISE_PYTHON_COMPILE", "0"),
        ("MISE_NODE_COMPILE", "0"),
        ("MISE_YES", "1"),
        ("MISE_PARANOID", "0"),
        ("MISE_DISABLE_BACKENDS", "asdf,vfox"),
        ("MISE_CHECK_FOR_NEW_VERSIONS", "never"),
        ("MISE_SELF_UPDATE_DISABLED", "1"),
        ("MISE_DATA_DIR", "/tmp/mise/data"),
        ("MISE_CACHE_DIR", "/tmp/mise/cache"),
        ("MISE_STATE_DIR", "/tmp/mise/state"),
        ("SSL_CERT_FILE", "/etc/ssl/certs/ca-certificates.crt"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect()
}

/// Hosts the provisioner guest reaches beyond the tool backends themselves: the version index mise consults and the sigstore TUF repo that attests python-build-standalone downloads.
pub fn baseline_egress_hosts() -> &'static [&'static str] {
    &["mise-versions.jdx.dev", "tuf-repo-cdn.sigstore.dev"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_parses_with_both_arches_pinned_everywhere() {
        let manifest = manifest();
        for arch in ["aarch64", "x86_64"] {
            let is_sha = |sha: &str| sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit());
            assert!(is_sha(&manifest.engine.sha256[arch]), "engine {arch}");
            assert!(is_sha(&manifest.static_curl.sha256[arch]), "curl {arch}");
            for companion in &manifest.companion {
                assert!(is_sha(&companion.sha256[arch]), "{} {arch}", companion.name);
            }
        }
        assert!(manifest.provisioner_rootfs.gnu.contains("@sha256:"));
        assert!(manifest.provisioner_rootfs.musl.contains("@sha256:"));
    }

    #[test]
    fn the_engine_url_names_the_pinned_static_musl_binary() {
        assert_eq!(
            engine_url(Arch::Aarch64),
            format!(
                "https://github.com/jdx/mise/releases/download/v{v}/mise-v{v}-linux-arm64-musl",
                v = engine_version()
            )
        );
        assert!(engine_url(Arch::X86_64).ends_with("linux-x64-musl"));
        assert_eq!(engine_sha256(Arch::Aarch64).len(), 64);
    }

    #[test]
    fn the_curl_url_names_the_pinned_static_binary() {
        assert!(curl_url(Arch::Aarch64).ends_with("/curl-aarch64"));
        assert!(curl_url(Arch::X86_64).ends_with("/curl-amd64"));
        assert_eq!(curl_sha256(Arch::X86_64).len(), 64);
    }

    #[test]
    fn the_companions_carry_the_ca_bundle_and_the_musl_node_runtime_deps() {
        let names: Vec<&str> = manifest()
            .companion
            .iter()
            .map(|companion| companion.name.as_str())
            .collect();
        for required in ["ca-certificates-bundle", "libstdc++", "libgcc", "bash"] {
            assert!(names.contains(&required), "missing companion {required}");
        }
        let ca = &manifest().companion[0];
        assert_eq!(
            companion_url(ca, Arch::X86_64),
            format!(
                "https://dl-cdn.alpinelinux.org/alpine/v3.20/main/x86_64/{}-{}.apk",
                ca.name, ca.version
            )
        );
        assert_eq!(companion_sha256(ca, Arch::Aarch64).len(), 64);
    }

    #[test]
    fn the_rootfs_flavors_map_musl_to_alpine_and_gnu_to_debian() {
        assert!(rootfs_reference(Libc::Musl).starts_with("docker.io/library/alpine@sha256:"));
        assert!(rootfs_reference(Libc::Gnu).starts_with("docker.io/library/debian@sha256:"));
    }

    #[test]
    fn the_provision_env_pins_the_fail_loud_knobs() {
        let env = provision_env();
        let get = |key: &str| {
            env.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
                .unwrap_or_else(|| panic!("missing {key}"))
        };
        assert_eq!(get("MISE_PYTHON_COMPILE"), "0");
        assert_eq!(get("MISE_NODE_COMPILE"), "0");
        assert_eq!(get("MISE_DISABLE_BACKENDS"), "asdf,vfox");
        assert_eq!(get("MISE_CHECK_FOR_NEW_VERSIONS"), "never");
        assert_eq!(get("SSL_CERT_FILE"), "/etc/ssl/certs/ca-certificates.crt");
    }

    #[test]
    fn the_baseline_egress_set_names_the_version_index_and_sigstore() {
        assert!(baseline_egress_hosts().contains(&"mise-versions.jdx.dev"));
        assert!(baseline_egress_hosts().contains(&"tuf-repo-cdn.sigstore.dev"));
    }
}
