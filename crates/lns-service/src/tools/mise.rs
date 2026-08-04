use std::collections::BTreeMap;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use serde::Deserialize;

use super::{Arch, Libc};

const MANIFEST: &str = include_str!("../../mise.toml");

const ALPINE_BRANCH: &str = "v3.20";

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub engine: Engine,
    pub provisioner_rootfs: ProvisionerRootfs,
    pub ca_bundle: CaBundle,
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
    pub gnu: BTreeMap<String, String>,
    pub musl: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct CaBundle {
    pub date: String,
    pub sha256: String,
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

impl Manifest {
    pub fn engine_url(&self, arch: Arch) -> String {
        let version = &self.engine.version;
        format!(
            "https://github.com/jdx/mise/releases/download/v{version}/mise-v{version}-linux-{}-musl",
            mise_release_arch(arch)
        )
    }

    pub fn engine_sha256(&self, arch: Arch) -> Result<&str> {
        pinned(&self.engine.sha256, arch, "engine sha256")
    }

    pub fn rootfs_reference(&self, libc: Libc, arch: Arch) -> Result<&str> {
        let flavor = match libc {
            Libc::Gnu => &self.provisioner_rootfs.gnu,
            Libc::Musl => &self.provisioner_rootfs.musl,
        };
        pinned(flavor, arch, "provisioner rootfs")
    }

    pub fn curl_url(&self, arch: Arch) -> String {
        format!(
            "https://github.com/moparisthebest/static-curl/releases/download/v{}/curl-{}",
            self.static_curl.version,
            release_arch(arch)
        )
    }

    pub fn curl_sha256(&self, arch: Arch) -> Result<&str> {
        pinned(&self.static_curl.sha256, arch, "static curl sha256")
    }
}

impl CaBundle {
    /// curl.se keeps every dated snapshot, so this URL stays fetchable for the life of the pin.
    pub fn url(&self) -> String {
        format!("https://curl.se/ca/cacert-{}.pem", self.date)
    }
}

pub fn companion_url(companion: &Companion, arch: Arch) -> String {
    format!(
        "https://dl-cdn.alpinelinux.org/alpine/{ALPINE_BRANCH}/main/{arch}/{}-{}.apk",
        companion.name, companion.version
    )
}

pub fn companion_sha256(companion: &Companion, arch: Arch) -> Result<&str> {
    pinned(&companion.sha256, arch, &companion.name)
}

/// The manifest is checked in, but a hand edit or an interrupted bump can leave an arch out; the service must say so rather than panic inside a run.
fn pinned<'a>(table: &'a BTreeMap<String, String>, arch: Arch, what: &str) -> Result<&'a str> {
    table
        .get(&arch.to_string())
        .map(String::as_str)
        .with_context(|| format!("mise.toml pins no {what} for {arch}"))
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

/// The fail-loud engine environment: compiling from source cannot work on a bare guest and buries the real error, `CI=1` silences the version phone-home, every mise path is pinned under /tmp, and the per-tool data and cache dirs are the driver's to set.
pub fn provision_env() -> Vec<(String, String)> {
    [
        ("MISE_PYTHON_COMPILE", "0"),
        ("MISE_NODE_COMPILE", "0"),
        ("MISE_YES", "1"),
        // Every backend that is not download-only: the snapshot records the download backend a tool is gated and audited against, so mise must not silently pick a different one (oxlint via npm, tokei via cargo, imagemagick via conda).
        (
            "MISE_DISABLE_BACKENDS",
            "asdf,vfox,npm,cargo,pipx,gem,go,dotnet,conda,spm",
        ),
        ("CI", "1"),
        ("HOME", "/tmp/mise/home"),
        ("MISE_STATE_DIR", "/tmp/mise/state"),
        ("MISE_CONFIG_DIR", "/tmp/mise/config"),
        ("SSL_CERT_FILE", lns_session::SYSTEM_CA_BUNDLE_PATH),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect()
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
        for arch in ["aarch64", "x86_64"] {
            assert!(manifest.provisioner_rootfs.gnu[arch].contains("@sha256:"));
            assert!(manifest.provisioner_rootfs.musl[arch].contains("@sha256:"));
        }
    }

    #[test]
    fn the_engine_url_names_the_pinned_static_musl_binary() {
        assert_eq!(
            manifest().engine_url(Arch::Aarch64),
            format!(
                "https://github.com/jdx/mise/releases/download/v{v}/mise-v{v}-linux-arm64-musl",
                v = engine_version()
            )
        );
        assert!(
            manifest()
                .engine_url(Arch::X86_64)
                .ends_with("linux-x64-musl")
        );
        assert_eq!(manifest().engine_sha256(Arch::Aarch64).unwrap().len(), 64);
    }

    #[test]
    fn the_curl_url_names_the_pinned_static_binary() {
        assert!(
            manifest()
                .curl_url(Arch::Aarch64)
                .ends_with("/curl-aarch64")
        );
        assert!(manifest().curl_url(Arch::X86_64).ends_with("/curl-amd64"));
        assert_eq!(manifest().curl_sha256(Arch::X86_64).unwrap().len(), 64);
    }

    #[test]
    fn the_companions_carry_the_musl_node_runtime_deps() {
        let names: Vec<&str> = manifest()
            .companion
            .iter()
            .map(|companion| companion.name.as_str())
            .collect();
        for required in ["libstdc++", "libgcc", "bash"] {
            assert!(names.contains(&required), "missing companion {required}");
        }
        assert!(
            !names.contains(&"ca-certificates-bundle"),
            "the CA store comes from a source that keeps superseded versions, not the apk branch head"
        );
        let lib = &manifest().companion[0];
        assert_eq!(
            companion_url(lib, Arch::X86_64),
            format!(
                "https://dl-cdn.alpinelinux.org/alpine/v3.20/main/x86_64/{}-{}.apk",
                lib.name, lib.version
            )
        );
        assert_eq!(companion_sha256(lib, Arch::Aarch64).unwrap().len(), 64);
    }

    #[test]
    fn the_ca_bundle_is_pinned_to_a_dated_snapshot_that_upstream_retains() {
        let ca = &manifest().ca_bundle;
        assert_eq!(
            ca.url(),
            format!("https://curl.se/ca/cacert-{}.pem", ca.date),
            "a dated snapshot never moves, so the pin cannot 404 when upstream refreshes"
        );
        assert_eq!(ca.sha256.len(), 64);
    }

    #[test]
    fn the_rootfs_flavors_map_musl_to_alpine_and_gnu_to_debian_per_arch() {
        for arch in [Arch::Aarch64, Arch::X86_64] {
            assert!(
                manifest()
                    .rootfs_reference(Libc::Musl, arch)
                    .unwrap()
                    .starts_with("docker.io/library/alpine@sha256:")
            );
            assert!(
                manifest()
                    .rootfs_reference(Libc::Gnu, arch)
                    .unwrap()
                    .starts_with("docker.io/library/debian@sha256:")
            );
        }
    }

    #[test]
    fn an_arch_the_manifest_does_not_pin_is_an_error_not_a_panic() {
        let manifest: Manifest = toml::from_str(
            r#"
            [engine]
            version = "1.0.0"
            [engine.sha256]
            aarch64 = "aa"
            [provisioner_rootfs.gnu]
            aarch64 = "debian@sha256:aa"
            [provisioner_rootfs.musl]
            aarch64 = "alpine@sha256:bb"
            [ca_bundle]
            date = "2026-07-16"
            sha256 = "aa"
            [static_curl]
            version = "8.0.0"
            [static_curl.sha256]
            aarch64 = "cc"
            "#,
        )
        .expect("a partial manifest still parses");
        let err = manifest.engine_sha256(Arch::X86_64).unwrap_err();
        assert!(
            format!("{err:#}").contains("pins no engine sha256 for x86_64"),
            "got: {err:#}"
        );
        assert!(manifest.curl_sha256(Arch::X86_64).is_err());
        assert!(manifest.rootfs_reference(Libc::Musl, Arch::X86_64).is_err());
        assert!(manifest.engine_sha256(Arch::Aarch64).is_ok());
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
        let disabled = get("MISE_DISABLE_BACKENDS");
        for backend in [
            "asdf", "vfox", "npm", "cargo", "pipx", "gem", "go", "dotnet", "conda", "spm",
        ] {
            assert!(
                disabled.split(',').any(|off| off == backend),
                "{backend} can install a tool the snapshot gated against another backend: {disabled}"
            );
        }
        for download_only in lns_artifact::tools::registry::SUPPORTED_BACKEND_KINDS {
            assert!(
                !disabled.split(',').any(|off| off == *download_only),
                "{download_only} is how tools are meant to install"
            );
        }
        assert_eq!(get("CI"), "1");
        assert_eq!(get("HOME"), "/tmp/mise/home");
        assert_eq!(get("SSL_CERT_FILE"), "/etc/ssl/certs/ca-certificates.crt");
    }
}
