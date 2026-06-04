use anyhow::{Context, Result, bail};
use flate2::read::MultiGzDecoder;
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};

mod real;
pub use real::ensure;

pub(crate) trait Fetcher: Send + Sync {
    fn fetch(&self, url: &str) -> impl std::future::Future<Output = Result<Vec<u8>>> + Send;
}

const ALPINE_VERSION: &str = "v3.20";
const ALPINE_REPO: &str = "main";

#[cfg(target_arch = "aarch64")]
const ARCH: &str = "aarch64";
#[cfg(target_arch = "x86_64")]
const ARCH: &str = "x86_64";

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
compile_error!(
    "lns only supports x86_64 and aarch64 hosts. Add per-arch sha256\n\
     pins for the Alpine apks below before enabling another arch."
);

#[cfg(target_arch = "aarch64")]
pub(super) const PACKAGES: &[PackageSpec] = &[PackageSpec {
    name: "busybox-static",
    version: "1.36.1-r31",
    sha256: "1d8e7a7fc2ed69bdb8eb7be9d962489c21a0f75b72d8a325437c8a09d4cfac70",
}];

#[cfg(target_arch = "x86_64")]
pub(super) const PACKAGES: &[PackageSpec] = &[PackageSpec {
    name: "busybox-static",
    version: "1.36.1-r31",
    sha256: "1ca8a2894b073edea2a04231f335ad48795f92617c5eacae59458c51202068ed",
}];

pub(super) struct PackageSpec {
    pub(super) name: &'static str,
    pub(super) version: &'static str,
    pub(super) sha256: &'static str,
}

#[derive(Debug)]
pub struct GuestTools {
    pub root: PathBuf,
}

pub(crate) async fn ensure_with<F: Fetcher>(
    fetcher: &F,
    cache_root: PathBuf,
    packages: &[PackageSpec],
) -> Result<GuestTools> {
    let apks_dir = cache_root.join("apks");
    let extracted = cache_root.join("extracted");
    tokio::fs::create_dir_all(&apks_dir).await?;

    if extracted.join(".ready").exists() {
        return Ok(GuestTools { root: extracted });
    }

    if extracted.exists() {
        tokio::fs::remove_dir_all(&extracted)
            .await
            .context("clearing partial guest-tools extract")?;
    }
    tokio::fs::create_dir_all(&extracted).await?;

    for spec in packages {
        let apk = ensure_apk_with(fetcher, &apks_dir, spec).await?;
        let bytes = tokio::fs::read(&apk).await?;
        let extracted_for = extracted.clone();
        tokio::task::spawn_blocking(move || extract_package(&extracted_for, &bytes)).await??;
    }

    tokio::fs::write(extracted.join(".ready"), b"").await?;
    Ok(GuestTools { root: extracted })
}

async fn ensure_apk_with<F: Fetcher>(
    fetcher: &F,
    apks_dir: &Path,
    spec: &PackageSpec,
) -> Result<PathBuf> {
    let file_name = format!("{}-{}.apk", spec.name, spec.version);
    let path = apks_dir.join(&file_name);

    if path.exists() {
        let bytes = tokio::fs::read(&path).await?;
        if sha256_hex(&bytes) == spec.sha256 {
            return Ok(path);
        }
        tokio::fs::remove_file(&path).await.ok();
    }

    let url = format!(
        "https://dl-cdn.alpinelinux.org/alpine/{ALPINE_VERSION}/{ALPINE_REPO}/{ARCH}/{file_name}"
    );
    let bytes = fetcher.fetch(&url).await?;

    let got = sha256_hex(&bytes);
    if got != spec.sha256 {
        bail!(
            "sha256 mismatch for {file_name}\n  expected {expected}\n  got      {got}\n\
             This either means the apk was tampered with in transit or the\n\
             pinned hash in guest_tools::PACKAGES is stale (Alpine bumped\n\
             a -rN suffix). Verify the hash from a trusted source before\n\
             updating the pin.",
            expected = spec.sha256,
        );
    }

    tokio::fs::write(&path, &bytes)
        .await
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

pub(crate) fn cache_subpath(cache_root: &Path) -> PathBuf {
    cache_root
        .join("guest-tools")
        .join(ALPINE_VERSION)
        .join(ARCH)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn extract_package(root: &Path, bytes: &[u8]) -> Result<()> {
    let cursor = Cursor::new(bytes);
    let decoder = MultiGzDecoder::new(cursor);
    let mut archive = tar::Archive::new(decoder);
    archive.set_overwrite(true);
    archive.set_preserve_permissions(true);

    for entry_result in archive.entries()? {
        let mut entry = entry_result?;
        let path = entry.path()?.into_owned();
        validate_archive_path(&path)?;
        if is_apk_metadata(&path) {
            continue;
        }
        let Some(mapped) = map_guest_tool_path(&path) else {
            continue;
        };
        let dest = root.join(mapped);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        entry
            .unpack(&dest)
            .with_context(|| format!("extracting {}", path.display()))?;
    }
    Ok(())
}

fn map_guest_tool_path(path: &Path) -> Option<PathBuf> {
    (path == Path::new("bin/busybox.static"))
        .then(|| Path::new("run/lns/guest-tools/bin/busybox").to_path_buf())
}

fn validate_archive_path(path: &Path) -> Result<()> {
    if path.components().any(|c| {
        matches!(
            c,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    }) {
        bail!(
            "refusing to unpack non-relative apk path {}",
            path.display()
        );
    }
    Ok(())
}

fn is_apk_metadata(path: &Path) -> bool {
    path.components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .is_some_and(|s| s.starts_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_least_one_package_is_pinned() {
        assert!(!PACKAGES.is_empty());
    }

    #[test]
    fn pinned_packages_have_consistent_format() {
        for spec in PACKAGES {
            assert!(!spec.name.is_empty(), "empty name in PACKAGES");
            assert!(!spec.version.is_empty(), "empty version for {}", spec.name);
            assert_eq!(
                spec.sha256.len(),
                64,
                "sha256 for {} wrong length: {:?}",
                spec.name,
                spec.sha256
            );
            assert!(
                spec.sha256
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "sha256 for {} is not lowercase hex: {:?}",
                spec.name,
                spec.sha256
            );
        }
    }

    #[test]
    fn alpine_release_pinned() {
        assert_eq!(ALPINE_VERSION, "v3.20");
        assert_eq!(ALPINE_REPO, "main");
    }

    #[test]
    fn map_guest_tool_path_relocates_static_busybox_under_run_lns() {
        assert_eq!(
            map_guest_tool_path(Path::new("bin/busybox.static")).as_deref(),
            Some(Path::new("run/lns/guest-tools/bin/busybox"))
        );
        assert!(map_guest_tool_path(Path::new("bin/busybox")).is_none());
        assert!(map_guest_tool_path(Path::new("bin/sh")).is_none());
        assert!(map_guest_tool_path(Path::new("usr/share/udhcpc/default.script")).is_none());
        assert!(map_guest_tool_path(Path::new("etc/hostname")).is_none());
    }

    #[test]
    fn validate_archive_path_accepts_relative_paths() {
        for p in ["usr/lib/libxtables.so.12", "bin/busybox", "a/b/c"] {
            validate_archive_path(Path::new(p)).expect(p);
        }
    }

    #[test]
    fn validate_archive_path_rejects_traversal_attempts() {
        for p in ["/etc/passwd", "../etc/passwd", "foo/../../etc/passwd", ".."] {
            let err = validate_archive_path(Path::new(p)).unwrap_err();
            assert!(
                format!("{err:#}").contains("non-relative"),
                "{p}: expected non-relative error, got {err:#}"
            );
        }
    }

    #[test]
    fn is_apk_metadata_matches_top_level_dotfiles_only() {
        for p in [
            ".PKGINFO",
            ".SIGN.RSA.alpine-devel.rsa.pub",
            ".trigger",
            ".post-install",
        ] {
            assert!(is_apk_metadata(Path::new(p)), "{p} should be metadata");
        }
        for p in [
            "etc/.something",
            "home/u/.bashrc",
            "bin/busybox",
            "usr/lib/libxtables.so.12",
        ] {
            assert!(!is_apk_metadata(Path::new(p)), "{p} should not be metadata");
        }
    }

    #[test]
    fn sha256_hex_emits_lowercase_64_char_hex() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let hex = sha256_hex(b"alpine apk bytes");
        assert_eq!(hex.len(), 64);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    fn write_tar_entry(builder: &mut tar::Builder<&mut Vec<u8>>, path: &str, contents: &[u8]) {
        let mut h = tar::Header::new_gnu();
        h.set_path(path).unwrap();
        h.set_size(contents.len() as u64);
        h.set_mode(0o644);
        h.set_uid(0);
        h.set_gid(0);
        h.set_mtime(0);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        builder.append(&h, std::io::Cursor::new(contents)).unwrap();
    }

    fn gzip_tar(write_entries: impl FnOnce(&mut tar::Builder<&mut Vec<u8>>)) -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            write_entries(&mut builder);
            builder.finish().unwrap();
        }
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&tar_bytes).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn extract_package_relocates_static_busybox_under_run_lns_guest_tools() {
        let bytes = gzip_tar(|b| {
            write_tar_entry(b, "bin/busybox.static", b"busybox-bin");
            write_tar_entry(b, "bin/sh", b"unwanted");
            write_tar_entry(b, ".PKGINFO", b"meta");
        });
        let dir = tempfile::TempDir::new().unwrap();
        extract_package(dir.path(), &bytes).unwrap();
        assert!(dir.path().join("run/lns/guest-tools/bin/busybox").is_file());
        assert!(!dir.path().join("run/lns/guest-tools/bin/sh").exists());
        assert!(
            !dir.path().join(".PKGINFO").exists(),
            "apk metadata skipped"
        );
    }

    use std::collections::HashMap;
    use std::sync::Mutex;

    struct FakeFetcher {
        responses: HashMap<String, Result<Vec<u8>, String>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeFetcher {
        fn new() -> Self {
            Self {
                responses: HashMap::new(),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn ok(mut self, url: impl Into<String>, bytes: Vec<u8>) -> Self {
            self.responses.insert(url.into(), Ok(bytes));
            self
        }
        fn err(mut self, url: impl Into<String>, msg: impl Into<String>) -> Self {
            self.responses.insert(url.into(), Err(msg.into()));
            self
        }
    }

    impl Fetcher for FakeFetcher {
        async fn fetch(&self, url: &str) -> Result<Vec<u8>> {
            self.calls.lock().unwrap().push(url.to_string());
            match self.responses.get(url) {
                Some(Ok(bytes)) => Ok(bytes.clone()),
                Some(Err(msg)) => Err(anyhow::anyhow!("{msg}")),
                None => Err(anyhow::anyhow!("FakeFetcher: no canned response for {url}")),
            }
        }
    }

    fn synthetic_apk() -> Vec<u8> {
        gzip_tar(|b| {
            write_tar_entry(b, "bin/busybox.static", b"busybox");
        })
    }

    fn matched_test_pin() -> (Vec<PackageSpec>, FakeFetcher) {
        let bytes = synthetic_apk();
        let sha = sha256_hex(&bytes);
        let spec = PackageSpec {
            name: "busybox-static",
            version: "test",
            sha256: Box::leak(sha.into_boxed_str()),
        };
        let url = format!(
            "https://dl-cdn.alpinelinux.org/alpine/{ALPINE_VERSION}/{ALPINE_REPO}/{ARCH}/busybox-static-test.apk",
        );
        let fetcher = FakeFetcher::new().ok(url, bytes);
        (vec![spec], fetcher)
    }

    #[tokio::test]
    async fn ensure_apk_with_bails_on_sha256_mismatch_with_actionable_message() {
        let spec = &PACKAGES[0];
        let url = format!(
            "https://dl-cdn.alpinelinux.org/alpine/{ALPINE_VERSION}/{ALPINE_REPO}/{ARCH}/{}-{}.apk",
            spec.name, spec.version
        );
        let fetcher = FakeFetcher::new().ok(url, b"wrong bytes for this pin".to_vec());
        let dir = tempfile::TempDir::new().unwrap();
        let err = ensure_apk_with(&fetcher, dir.path(), spec)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("sha256 mismatch"), "{msg}");
        assert!(msg.contains(&format!("{}-{}.apk", spec.name, spec.version)));
        assert!(msg.contains("expected"));
        assert!(msg.contains("got"));
    }

    #[tokio::test]
    async fn ensure_apk_with_short_circuits_when_cache_matches_pin() {
        let dir = tempfile::TempDir::new().unwrap();
        let payload = b"cached-already".to_vec();
        let payload_sha = sha256_hex(&payload);
        let spec = PackageSpec {
            name: "busybox-static",
            version: "0-r0",
            sha256: Box::leak(payload_sha.into_boxed_str()),
        };
        let path = dir
            .path()
            .join(format!("{}-{}.apk", spec.name, spec.version));
        tokio::fs::write(&path, &payload).await.unwrap();

        let fetcher = FakeFetcher::new();
        let resolved = ensure_apk_with(&fetcher, dir.path(), &spec).await.unwrap();
        assert_eq!(resolved, path);
        assert!(
            fetcher.calls.lock().unwrap().is_empty(),
            "cache short-circuit must skip the network"
        );
    }

    #[tokio::test]
    async fn ensure_apk_with_replaces_cache_when_stale_then_fetches() {
        let dir = tempfile::TempDir::new().unwrap();
        let stale = b"stale half-download".to_vec();
        let fresh = b"fresh body returned by registry".to_vec();
        let fresh_sha = sha256_hex(&fresh);
        let spec = PackageSpec {
            name: "busybox-static",
            version: "0-r0",
            sha256: Box::leak(fresh_sha.into_boxed_str()),
        };
        let path = dir
            .path()
            .join(format!("{}-{}.apk", spec.name, spec.version));
        tokio::fs::write(&path, &stale).await.unwrap();
        let url = format!(
            "https://dl-cdn.alpinelinux.org/alpine/{ALPINE_VERSION}/{ALPINE_REPO}/{ARCH}/{}-{}.apk",
            spec.name, spec.version
        );
        let fetcher = FakeFetcher::new().ok(url, fresh.clone());
        let resolved = ensure_apk_with(&fetcher, dir.path(), &spec).await.unwrap();
        let on_disk = tokio::fs::read(&resolved).await.unwrap();
        assert_eq!(on_disk, fresh, "stale cache replaced with fresh bytes");
    }

    #[tokio::test]
    async fn ensure_apk_with_propagates_fetch_error_with_url_context() {
        let spec = &PACKAGES[0];
        let url = format!(
            "https://dl-cdn.alpinelinux.org/alpine/{ALPINE_VERSION}/{ALPINE_REPO}/{ARCH}/{}-{}.apk",
            spec.name, spec.version
        );
        let fetcher = FakeFetcher::new().err(url, "dl-cdn went down");
        let dir = tempfile::TempDir::new().unwrap();
        let err = ensure_apk_with(&fetcher, dir.path(), spec)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("dl-cdn went down"));
    }

    #[tokio::test]
    async fn ensure_with_short_circuits_when_ready_marker_present() {
        let dir = tempfile::TempDir::new().unwrap();
        let extracted = dir.path().join("extracted");
        tokio::fs::create_dir_all(&extracted).await.unwrap();
        tokio::fs::write(extracted.join(".ready"), b"")
            .await
            .unwrap();

        let fetcher = FakeFetcher::new();
        let result = ensure_with(&fetcher, dir.path().to_path_buf(), PACKAGES)
            .await
            .unwrap();
        assert_eq!(result.root, extracted);
        assert!(
            fetcher.calls.lock().unwrap().is_empty(),
            ".ready marker must skip every apk fetch"
        );
    }

    #[tokio::test]
    async fn ensure_with_clears_partial_extract_before_redoing_work() {
        let dir = tempfile::TempDir::new().unwrap();
        let extracted = dir.path().join("extracted");
        tokio::fs::create_dir_all(&extracted).await.unwrap();
        tokio::fs::write(extracted.join("stale.txt"), b"old")
            .await
            .unwrap();

        let spec = &PACKAGES[0];
        let url = format!(
            "https://dl-cdn.alpinelinux.org/alpine/{ALPINE_VERSION}/{ALPINE_REPO}/{ARCH}/{}-{}.apk",
            spec.name, spec.version
        );
        let fetcher = FakeFetcher::new().err(url, "boom");
        let _err = ensure_with(&fetcher, dir.path().to_path_buf(), PACKAGES)
            .await
            .unwrap_err();
        assert!(
            !extracted.join("stale.txt").exists(),
            "stale extracted/ must be cleared before redoing work"
        );
    }

    #[tokio::test]
    async fn ensure_with_completes_end_to_end_writing_ready_marker_after_full_extract() {
        let (packages, fetcher) = matched_test_pin();
        let dir = tempfile::TempDir::new().unwrap();
        let result = ensure_with(&fetcher, dir.path().to_path_buf(), &packages)
            .await
            .expect("ensure_with happy path");
        assert_eq!(result.root, dir.path().join("extracted"));
        assert!(
            result.root.join(".ready").exists(),
            ".ready marker must be written so the next call short-circuits"
        );
        assert!(
            result
                .root
                .join("run/lns/guest-tools/bin/busybox")
                .is_file(),
            "busybox landed under run/lns/guest-tools"
        );
        assert!(
            !result.root.join("lib").exists(),
            "nothing of ours lands in image-owned /lib anymore"
        );
    }

    #[tokio::test]
    async fn fake_fetcher_unmatched_url_reports_no_canned_response() {
        let fetcher = FakeFetcher::new();
        let err = fetcher.fetch("https://nope/").await.unwrap_err();
        assert!(format!("{err:#}").contains("no canned response"));
    }

    #[test]
    fn cache_subpath_layout_pins_alpine_release_under_per_arch_dir() {
        let base = Path::new("/cache");
        let p = cache_subpath(base);
        assert_eq!(p, base.join("guest-tools").join(ALPINE_VERSION).join(ARCH));
    }
}
