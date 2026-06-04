// Layer-1 oracle: builds a composefs descriptor with the pure-Rust EROFS writer
// and mounts it (erofs + overlay redirect/metacopy) inside a privileged Alpine
// container to prove the descriptor + content-addressed blob chain resolves.
// #[ignore]d (real docker on the host) — run manually with:
//   cargo test -p e2e-tests --test composefs_oracle -- --ignored

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

use sha2::Digest;

use lns_service::composefs::oci::type_bits;
use lns_service::composefs::vendor::generic_tree::{Inode, LeafContent, Stat};
use lns_service::composefs::vendor::tree::{FileSystem, RegularFile};
use lns_service::composefs::{Sha256Digest, mkfs_erofs};

fn find_docker() -> Option<PathBuf> {
    if let Ok(out) = Command::new("which").arg("docker").output()
        && out.status.success()
    {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(PathBuf::from(s));
        }
    }
    for p in ["/usr/local/bin/docker", "/opt/homebrew/bin/docker"] {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    None
}

fn project_tempdir() -> tempfile::TempDir {
    let parent = dirs::home_dir()
        .map(|h| h.join(".cache/lns-composefs-test"))
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&parent).expect("create parent tempdir");
    tempfile::TempDir::new_in(&parent).expect("tempdir")
}

fn dir_stat() -> Stat {
    Stat {
        st_mode: type_bits(libc::S_IFDIR) | 0o755,
        st_uid: 0,
        st_gid: 0,
        st_mtim_sec: 0,
        xattrs: BTreeMap::new(),
    }
}

fn file_stat() -> Stat {
    Stat {
        st_mode: type_bits(libc::S_IFREG) | 0o644,
        st_uid: 0,
        st_gid: 0,
        st_mtim_sec: 0,
        xattrs: BTreeMap::new(),
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[test]
#[ignore]
fn composefs_descriptor_mounts_and_resolves_redirects() {
    let Some(docker) = find_docker() else {
        eprintln!("SKIP: docker not on host");
        return;
    };

    let work = project_tempdir();
    let work_path = work.path();

    let body = b"hello composefs integration test\n";
    let raw_digest: [u8; 32] = sha2::Sha256::digest(body).into();
    let hex = hex_lower(&raw_digest);
    let digest = Sha256Digest::from(raw_digest);

    let mut fs = FileSystem::<Sha256Digest>::new(dir_stat());
    let leaf_id = fs.push_leaf(
        file_stat(),
        LeafContent::Regular(RegularFile::External(digest, body.len() as u64)),
    );
    fs.root
        .insert(OsStr::new("greeting.txt"), Inode::leaf(leaf_id));

    let descriptor = mkfs_erofs(&fs);
    let descriptor_path = work_path.join("descriptor.erofs");
    std::fs::write(&descriptor_path, &descriptor[..]).expect("write descriptor");

    let content_dir = work_path.join("content");
    let blob_dir = content_dir.join("sha256");
    std::fs::create_dir_all(&blob_dir).expect("create blob dir");
    std::fs::write(blob_dir.join(&hex), body).expect("write blob");

    let script = r#"
set -eu
echo "---kernel---"
uname -a
apk add --no-cache attr >/dev/null 2>&1 || true
mkdir -p /mnt/meta /merged
mount -t tmpfs tmpfs /tmp
mkdir -p /tmp/upper /tmp/work
echo "---mounting-erofs---"
mount -t erofs -o ro,loop /work/descriptor.erofs /mnt/meta
echo "---meta-listing---"
ls -la /mnt/meta
echo "---greeting-xattrs-on-meta---"
getfattr -d -m '^trusted' --absolute-names /mnt/meta/greeting.txt 2>&1 || true
echo "---content-listing---"
ls -la /work/content/sha256
echo "---mounting-overlay-with-::---"
set +e
mount -t overlay overlay \
    -o "lowerdir=/mnt/meta::/work/content,upperdir=/tmp/upper,workdir=/tmp/work,redirect_dir=on,metacopy=on" \
    /merged
rc=$?
set -e
if [ $rc -ne 0 ]; then
    echo "---::-mount-failed, trying read-only fallback---"
    mount -t overlay overlay \
        -o "lowerdir=/mnt/meta::/work/content,redirect_dir=on,metacopy=on" \
        /merged
fi
echo "---merged-ls---"
ls /merged
echo "---greeting.txt-via-merge---"
cat /merged/greeting.txt
echo "INTEGRATION_OK"
"#;

    let out = Command::new(&docker)
        .args([
            "run",
            "--rm",
            "--privileged",
            "-v",
            &format!("{}:/work", work_path.display()),
            "alpine:3.20",
            "sh",
            "-c",
            script,
        ])
        .output()
        .expect("run docker");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    eprintln!("stdout:\n{stdout}");
    if !stderr.is_empty() {
        eprintln!("stderr:\n{stderr}");
    }
    assert!(
        out.status.success(),
        "docker exited non-zero — composefs+overlay chain likely broken"
    );
    assert!(
        stdout.contains("hello composefs integration test"),
        "merged view didn't surface the redirect-resolved content; \
         likely the descriptor's xattrs don't match Sha256Digest::to_object_pathname"
    );
    assert!(
        stdout.contains("INTEGRATION_OK"),
        "container didn't reach the sentinel — earlier step failed"
    );
}
