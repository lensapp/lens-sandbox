use anyhow::{Result, bail};
use std::path::PathBuf;

use crate::download::{PinnedArtifact, RealFetcher, RealFs, ensure_pinned};

pub const KERNEL_VERSION: &str = env!("KERNEL_VERSION");

const KERNEL_SHA256: &str = env!("KERNEL_SHA256");

#[cfg(target_arch = "aarch64")]
const ARCH: &str = "aarch64";

#[cfg(target_arch = "x86_64")]
const ARCH: &str = "x86_64";

const CDN_BASE: &str = "https://get.lns.run";

const MAX_KERNEL_BYTES: u64 = 64 * 1024 * 1024;

pub async fn ensure() -> Result<PathBuf> {
    ensure_with(|k| std::env::var_os(k)).await
}

async fn ensure_with(env_get: impl Fn(&str) -> Option<std::ffi::OsString>) -> Result<PathBuf> {
    if let Some(override_path) = env_get("LNS_KERNEL_PATH") {
        let p = PathBuf::from(override_path);
        if !p.is_file() {
            bail!(
                "LNS_KERNEL_PATH={} is not a regular file. Set the env var to \
                 a host-readable uncompressed Linux kernel `Image`, or unset \
                 it to fall back to the CDN-published Kata {KERNEL_VERSION}.",
                p.display()
            );
        }
        return Ok(p);
    }

    let cache = crate::cache::root()?.join("kernel");
    let cdn_base = env_get("LNS_KERNEL_CDN")
        .and_then(|v| v.into_string().ok())
        .unwrap_or_else(|| CDN_BASE.to_string());
    let filename = format!("Image-{KERNEL_VERSION}-{ARCH}");
    let url = format!("{cdn_base}/lns-kernel-{KERNEL_VERSION}-{ARCH}");
    let label = format!("guest kernel v{KERNEL_VERSION}");
    ensure_pinned(
        &RealFetcher {
            max_bytes: MAX_KERNEL_BYTES,
        },
        &RealFs,
        &cache,
        &PinnedArtifact {
            filename: &filename,
            url: &url,
            sha256: KERNEL_SHA256,
            mode: None,
            label: &label,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_tracing_capture() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let subscriber = tracing_subscriber::fmt()
                .with_writer(std::io::sink)
                .with_ansi(false)
                .with_max_level(tracing::Level::TRACE)
                .finish();
            tracing::subscriber::set_global_default(subscriber).ok();
        });
    }

    #[test]
    fn kernel_version_has_expected_shape() {
        let parts: Vec<&str> = KERNEL_VERSION.splitn(2, '-').collect();
        let parts_msg = format!("KERNEL_VERSION={KERNEL_VERSION:?} must be <semver>-<buildnum>");
        assert_eq!(parts.len(), 2, "{parts_msg}");
        let semver_str = parts[0];
        let build_str = parts[1];
        let semver_parts: Vec<&str> = semver_str.split('.').collect();
        let semver_msg =
            format!("KERNEL_VERSION semver portion {semver_str:?} must be major.minor.patch");
        assert_eq!(semver_parts.len(), 3, "{semver_msg}");
        let all_digits = semver_parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
        let parts_msg =
            format!("KERNEL_VERSION semver parts {semver_parts:?} must all be non-empty integers");
        assert!(all_digits, "{parts_msg}");
        let build_cond = !build_str.is_empty() && build_str.chars().all(|c| c.is_ascii_digit());
        let build_msg =
            format!("KERNEL_VERSION build part {build_str:?} must be a non-empty integer");
        assert!(build_cond, "{build_msg}");
    }

    #[test]
    fn pin_is_64_hex_chars() {
        assert_eq!(KERNEL_SHA256.len(), 64);
        assert!(KERNEL_SHA256.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn cdn_url_has_expected_shape() {
        let url = format!("{CDN_BASE}/lns-kernel-{KERNEL_VERSION}-{ARCH}");
        assert!(
            url.starts_with("https://get.lns.run/lns-kernel-"),
            "url {url:?} should start with the CDN base + artifact prefix"
        );
        assert!(
            url.ends_with(&format!("-{ARCH}")),
            "url {url:?} should end with -{ARCH}"
        );
        assert!(
            url.contains(KERNEL_VERSION),
            "url {url:?} should embed KERNEL_VERSION={KERNEL_VERSION}"
        );
    }

    #[tokio::test]
    async fn lns_kernel_path_env_var_overrides() {
        init_tracing_capture();
        let workspace_toml = std::env::current_dir()
            .unwrap()
            .ancestors()
            .find(|p| {
                p.join("Cargo.toml").is_file() && {
                    std::fs::read_to_string(p.join("Cargo.toml"))
                        .ok()
                        .is_some_and(|c| c.contains("[workspace]"))
                }
            })
            .map(|p| p.join("Cargo.toml"))
            .expect("workspace Cargo.toml");
        let target = workspace_toml.clone();
        let stub =
            move |k: &str| (k == "LNS_KERNEL_PATH").then(|| target.as_os_str().to_os_string());
        let resolved = ensure_with(stub).await.expect("ensure with override");
        assert_eq!(resolved, workspace_toml);
    }

    #[tokio::test]
    async fn lns_kernel_path_env_var_rejects_missing_file() {
        init_tracing_capture();
        let stub = |k: &str| {
            (k == "LNS_KERNEL_PATH").then(|| std::ffi::OsString::from("/does/not/exist/Image"))
        };
        let err = ensure_with(stub).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("LNS_KERNEL_PATH"));
        assert!(msg.contains("/does/not/exist/Image"));
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn ensure_routes_through_lns_kernel_path_override() {
        init_tracing_capture();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let _g = crate::test_env::EnvVarGuard::set("LNS_KERNEL_PATH", &path);
        let resolved = ensure().await;
        assert_eq!(resolved.expect("override should succeed"), path);
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn ensure_production_wiring_uses_lns_kernel_cdn_and_rejects_sha_mismatch() {
        init_tracing_capture();
        let server = wiremock::MockServer::start().await;
        let expected_path = format!("/lns-kernel-{KERNEL_VERSION}-{ARCH}");
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(expected_path.clone()))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_bytes(b"not the real kernel".as_slice()),
            )
            .expect(1)
            .mount(&server)
            .await;

        let cache_root = tempfile::TempDir::new().unwrap();
        let _path = crate::test_env::EnvVarGuard::unset("LNS_KERNEL_PATH");
        let _cdn = crate::test_env::EnvVarGuard::set("LNS_KERNEL_CDN", server.uri());
        let _home = crate::test_env::EnvVarGuard::set("HOME", cache_root.path());
        let _xdg =
            crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", cache_root.path().join("xdg"));
        let result = ensure().await;

        let err = result.expect_err("wiremock bytes won't match KERNEL_SHA256");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("sha256 mismatch"),
            "sha mismatch surface: {msg}"
        );
        assert!(
            msg.contains(&format!("{}{}", server.uri(), expected_path)),
            "URL must appear in error message: {msg}"
        );
        assert!(
            msg.contains("Refusing to install"),
            "actionable verb: {msg}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn real_fetcher_wraps_http_error_with_url_context() {
        init_tracing_capture();
        let server = wiremock::MockServer::start().await;
        let expected_path = format!("/lns-kernel-{KERNEL_VERSION}-{ARCH}");
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(expected_path.clone()))
            .respond_with(wiremock::ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let cache_root = tempfile::TempDir::new().unwrap();
        let _path = crate::test_env::EnvVarGuard::unset("LNS_KERNEL_PATH");
        let _cdn = crate::test_env::EnvVarGuard::set("LNS_KERNEL_CDN", server.uri());
        let _home = crate::test_env::EnvVarGuard::set("HOME", cache_root.path());
        let _xdg =
            crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", cache_root.path().join("xdg"));
        let result = ensure().await;

        let err = result.expect_err("503 must bail");
        let msg = format!("{err:#}");
        assert!(msg.contains("HTTP error from"), "context: {msg}");
        assert!(
            msg.contains(&format!("{}{}", server.uri(), expected_path)),
            "URL in error: {msg}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn real_fetcher_wraps_transport_error_with_url_context() {
        init_tracing_capture();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let cache_root = tempfile::TempDir::new().unwrap();
        let _path = crate::test_env::EnvVarGuard::unset("LNS_KERNEL_PATH");
        let _cdn = crate::test_env::EnvVarGuard::set("LNS_KERNEL_CDN", format!("http://{addr}"));
        let _home = crate::test_env::EnvVarGuard::set("HOME", cache_root.path());
        let _xdg =
            crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", cache_root.path().join("xdg"));
        let result = ensure().await;

        let err = result.expect_err("connect-refused must bail");
        let msg = format!("{err:#}");
        assert!(msg.contains("downloading"), "context: {msg}");
        assert!(
            msg.contains(&format!("http://{addr}")),
            "URL in error: {msg}"
        );
    }
}
