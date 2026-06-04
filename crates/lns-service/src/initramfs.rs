use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::write::GzEncoder;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::cpio::Initramfs;
use crate::guest_tools::GuestTools;

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
compile_error!(
    "lns only supports x86_64 and aarch64 hosts. Add per-arch sha256\n\
     pins in guest_tools::PACKAGES before enabling another arch."
);

const EMBEDDED_LNS_INIT: &[u8] = include_bytes!(env!("LNS_INIT_BIN_EMBEDDED"));

const EMBEDDED_LNS_SESSION_BROKER: &[u8] = include_bytes!(env!("LNS_SESSION_BROKER_BIN_EMBEDDED"));

pub async fn build(_tools: &GuestTools) -> Result<PathBuf> {
    let init_bytes = resolve_lns_init_bytes(EMBEDDED_LNS_INIT, |k| std::env::var_os(k))?;
    let broker_bytes = resolve_broker_bytes(EMBEDDED_LNS_SESSION_BROKER, |k| std::env::var_os(k))?;
    let key = content_hash(&init_bytes, &broker_bytes);
    let cache_dir = crate::cache::root()?.join("initramfs").join(&key[..16]);
    let path = cache_dir.join("initramfs.cpio.gz");
    if path.exists() {
        return Ok(path);
    }
    let bytes =
        tokio::task::spawn_blocking(move || encode_cpio(&init_bytes, &broker_bytes)).await??;
    tokio::fs::create_dir_all(&cache_dir).await?;
    tokio::fs::write(&path, &bytes)
        .await
        .with_context(|| format!("writing {}", path.display()))?;
    tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).await?;
    Ok(path)
}

fn resolve_lns_init_bytes(
    embedded: &[u8],
    env_get: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<Vec<u8>> {
    if let Some(env_val) = env_get("LNS_INIT_BIN").filter(|v| !v.is_empty()) {
        let p = PathBuf::from(env_val);
        if !p.is_file() {
            bail!(
                "LNS_INIT_BIN points at {} which is not a regular file. Either build with `cargo build -p lns-init --profile release-init --target aarch64-unknown-linux-musl`, or unset LNS_INIT_BIN to use the embedded binary.",
                p.display()
            );
        }
        return std::fs::read(&p).with_context(|| format!("reading {}", p.display()));
    }
    if embedded.is_empty() {
        bail!(
            "embedded lns-init is empty — build.rs saw LNS_INIT_BIN=<path> at compile time and skipped the cross-build, but the env var is now unset at runtime. Either set LNS_INIT_BIN again, or rebuild lns-cli without it (`unset LNS_INIT_BIN && cargo build -p lns-cli --release`)."
        );
    }
    let bytes = embedded.to_vec();
    Ok(bytes)
}

fn encode_cpio(init_bytes: &[u8], broker_bytes: &[u8]) -> Result<Vec<u8>> {
    let mut fs = Initramfs::new();

    for d in [
        "proc",
        "sys",
        "dev",
        "mnt",
        "mnt/upper",
        "newroot",
        "newroot/old-root",
        "content",
        "composefs-meta",
        "old-root",
    ] {
        fs.add_dir(d, 0o755);
    }
    fs.add_file("init", init_bytes, 0o755);
    fs.add_file("init-broker", broker_bytes, 0o755);
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    fs.finish(&mut gz)?;
    gz.finish().context("gzipping initramfs")
}

fn resolve_broker_bytes(
    embedded: &[u8],
    env_get: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<Vec<u8>> {
    if let Some(env_val) = env_get("LNS_SESSION_BROKER_BIN").filter(|v| !v.is_empty()) {
        let p = PathBuf::from(env_val);
        if !p.is_file() {
            bail!(
                "LNS_SESSION_BROKER_BIN points at {} which is not a regular file. Either build with `cargo build -p lns-session-broker --profile release-init --target aarch64-unknown-linux-musl`, or unset LNS_SESSION_BROKER_BIN to use the embedded binary.",
                p.display()
            );
        }
        return std::fs::read(&p).with_context(|| format!("reading {}", p.display()));
    }
    if embedded.is_empty() {
        bail!(
            "embedded lns-session-broker is empty — build.rs saw LNS_SESSION_BROKER_BIN=<path> at compile time and skipped the cross-build, but the env var is now unset at runtime. Either set LNS_SESSION_BROKER_BIN again, or rebuild without it (`unset LNS_SESSION_BROKER_BIN && cargo build -p lns-service --release`)."
        );
    }
    Ok(embedded.to_vec())
}

fn content_hash(init_bytes: &[u8], broker_bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(init_bytes);
    h.update(broker_bytes);
    format!("{:x}", h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_INIT: &[u8] = b"\x7fELF synthetic lns-init body, host tests";
    const FAKE_BROKER: &[u8] = b"\x7fELF synthetic lns-session-broker body";

    #[test]
    fn encode_cpio_produces_gzipped_archive() {
        let bytes = encode_cpio(FAKE_INIT, FAKE_BROKER).expect("encode");
        assert_eq!(&bytes[..2], &[0x1f, 0x8b]);
        assert!(bytes.len() > 200);
    }

    #[test]
    fn resolve_lns_init_bytes_returns_embedded_when_no_env() {
        let bytes = resolve_lns_init_bytes(FAKE_INIT, |_| None).expect("embedded bytes");
        assert_eq!(bytes, FAKE_INIT);
    }

    #[test]
    fn resolve_lns_init_bytes_bails_when_embedded_empty_and_no_env() {
        let err = resolve_lns_init_bytes(b"", |_| None).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("embedded lns-init is empty"), "msg: {msg}");
    }

    #[test]
    fn resolve_lns_init_bytes_honours_env_override() {
        let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let stub = |_: &str| Some(target.as_os_str().to_os_string());
        let bytes = resolve_lns_init_bytes(FAKE_INIT, stub).expect("read override");
        let expected = std::fs::read(&target).unwrap();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn resolve_lns_init_bytes_treats_empty_env_as_unset() {
        let stub = |_: &str| Some(std::ffi::OsString::from(""));
        let bytes = resolve_lns_init_bytes(FAKE_INIT, stub).expect("embedded bytes");
        assert_eq!(bytes, FAKE_INIT);
    }

    #[test]
    fn resolve_lns_init_bytes_rejects_missing_env_target() {
        let stub = |_: &str| Some(std::ffi::OsString::from("/nope/never/lns-init"));
        let err = resolve_lns_init_bytes(FAKE_INIT, stub).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("LNS_INIT_BIN"));
        assert!(msg.contains("/nope/never/lns-init"));
    }

    #[test]
    fn resolve_broker_bytes_returns_embedded_when_no_env() {
        let bytes = resolve_broker_bytes(FAKE_BROKER, |_| None).expect("embedded bytes");
        assert_eq!(bytes, FAKE_BROKER);
    }

    #[test]
    fn resolve_broker_bytes_bails_when_embedded_empty_and_no_env() {
        let err = resolve_broker_bytes(b"", |_| None).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("embedded lns-session-broker is empty"));
    }

    #[test]
    fn resolve_broker_bytes_honours_env_override() {
        let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let stub = |_: &str| Some(target.as_os_str().to_os_string());
        let bytes = resolve_broker_bytes(FAKE_BROKER, stub).expect("read override");
        let expected = std::fs::read(&target).unwrap();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn resolve_broker_bytes_treats_empty_env_as_unset() {
        let stub = |_: &str| Some(std::ffi::OsString::from(""));
        let bytes = resolve_broker_bytes(FAKE_BROKER, stub).expect("embedded bytes");
        assert_eq!(bytes, FAKE_BROKER);
    }

    #[test]
    fn resolve_broker_bytes_rejects_missing_env_target() {
        let stub = |_: &str| Some(std::ffi::OsString::from("/nope/never/lns-session-broker"));
        let err = resolve_broker_bytes(FAKE_BROKER, stub).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("LNS_SESSION_BROKER_BIN"));
        assert!(msg.contains("/nope/never/lns-session-broker"));
    }

    #[test]
    fn content_hash_is_deterministic_and_distinct_per_input() {
        let a = content_hash(b"init", b"broker");
        let b = content_hash(b"init", b"broker");
        let c = content_hash(b"init", b"other");
        let d = content_hash(b"other", b"broker");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
        assert_eq!(a.len(), 64, "sha256 hex is 64 chars");
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn build_writes_cpio_to_cache_and_second_call_short_circuits() {
        let tmphome = tempfile::TempDir::new().unwrap();
        let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let _home = crate::test_env::EnvVarGuard::set("HOME", tmphome.path());
        let _xdg =
            crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", tmphome.path().join(".cache"));
        let _init = crate::test_env::EnvVarGuard::set("LNS_INIT_BIN", &target);
        let _broker = crate::test_env::EnvVarGuard::set("LNS_SESSION_BROKER_BIN", &target);

        let tools = GuestTools {
            root: tmphome.path().join("guest-tools-unused"),
        };
        let p1 = build(&tools).await.expect("first build succeeds");
        let p2 = build(&tools)
            .await
            .expect("second build short-circuits on cache");

        assert!(p1.exists());
        assert_eq!(
            p1, p2,
            "second call must hit the cache and return the same path"
        );
        let name = p1.file_name().unwrap().to_str().unwrap();
        assert_eq!(name, "initramfs.cpio.gz");
        #[cfg(unix)]
        {
            let mode = std::fs::metadata(&p1).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o644);
        }
    }
}
