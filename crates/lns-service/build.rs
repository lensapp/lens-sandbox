use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const TARGET: &str = "aarch64-unknown-linux-musl";
const PROFILE: &str = "release-init";

const STATIC_NFT_VERSION: &str = "1.1.5";

#[derive(Deserialize)]
struct KernelManifest {
    current: KernelCurrent,
}

#[derive(Deserialize)]
struct KernelCurrent {
    kernel_filename: String,
    published_version: String,
    sha256: BTreeMap<String, String>,
}

fn main() {
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo");
    let workspace = PathBuf::from(&manifest_dir)
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root above crates/lns-service")
        .to_path_buf();

    emit_kernel_pin(&PathBuf::from(&manifest_dir).join("kernels.toml"));

    for path in [
        "crates/lns-init/src",
        "crates/lns-init/Cargo.toml",
        "crates/lns-session-broker/src",
        "crates/lns-session-broker/Cargo.toml",
        "crates/lns-session/src",
        "crates/lns-session/Cargo.toml",
        "crates/lns-supervisor/src",
        "crates/lns-supervisor/Cargo.toml",
    ] {
        println!("cargo:rerun-if-changed={}", workspace.join(path).display());
    }
    println!("cargo:rerun-if-env-changed=LNS_INIT_BIN");
    println!("cargo:rerun-if-env-changed=LNS_SESSION_BROKER_BIN");
    println!("cargo:rerun-if-env-changed=LNS_NFT_BIN");
    println!("cargo:rerun-if-env-changed=LNS_SUPERVISOR_BIN");

    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join("target"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR set by cargo"));

    stage_with_optional_override(
        &workspace,
        &target_dir,
        &out_dir,
        "lns-init",
        "LNS_INIT_BIN",
        "LNS_INIT_BIN_EMBEDDED",
    );

    stage_with_optional_override(
        &workspace,
        &target_dir,
        &out_dir,
        "lns-session-broker",
        "LNS_SESSION_BROKER_BIN",
        "LNS_SESSION_BROKER_BIN_EMBEDDED",
    );

    stage_with_optional_override(
        &workspace,
        &target_dir,
        &out_dir,
        "lns-supervisor",
        "LNS_SUPERVISOR_BIN",
        "LNS_SUPERVISOR_BIN_EMBEDDED",
    );

    stage_static_nft(&workspace, &out_dir);
}

fn stage_static_nft(workspace: &Path, out_dir: &Path) {
    if std::env::var_os("LNS_NFT_BIN").is_some_and(|v| !v.is_empty()) {
        let empty = out_dir.join("nft.empty");
        std::fs::write(&empty, []).expect("write empty nft placeholder");
        println!("cargo:rustc-env=LNS_NFT_BIN_EMBEDDED={}", empty.display());
        println!(
            "cargo:warning=lns-service build.rs: LNS_NFT_BIN is set; \
             skipping static-nft embed (override will be read at runtime)."
        );
        return;
    }

    let target_arch =
        std::env::var("CARGO_CFG_TARGET_ARCH").expect("CARGO_CFG_TARGET_ARCH set by cargo");
    let arch = match target_arch.as_str() {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => panic!(
            "static-nft embed: unsupported target_arch=\"{other}\" \
             (lns supports aarch64 and x86_64 hosts only)"
        ),
    };
    let asset = format!("nft-{STATIC_NFT_VERSION}-linux-{arch}-musl");
    let src = workspace.join("vendor").join("static-nft").join(&asset);
    println!("cargo:rerun-if-changed={}", src.display());

    if !src.is_file() {
        panic!(
            "static-nft embed: {} is missing. Run \
             `scripts/build-static-nft.sh linux/{arch}` (Docker required) \
             to produce it, or set LNS_NFT_BIN=/path/to/static/nft to bypass \
             the embed for dev iteration.",
            src.display()
        );
    }

    let dst = out_dir.join(&asset);
    std::fs::copy(&src, &dst).unwrap_or_else(|e| {
        panic!(
            "copying static nft from {} to {}: {e}",
            src.display(),
            dst.display()
        )
    });

    const MIN_NFT_BYTES: u64 = 500_000;
    let dst_size = std::fs::metadata(&dst)
        .unwrap_or_else(|e| panic!("stat'ing {}: {e}", dst.display()))
        .len();
    if dst_size < MIN_NFT_BYTES {
        panic!(
            "static-nft at {} is suspiciously small ({} bytes; expected > {}). \
             Re-run scripts/build-static-nft.sh to refresh the committed binary.",
            dst.display(),
            dst_size,
            MIN_NFT_BYTES,
        );
    }

    println!("cargo:rustc-env=LNS_NFT_BIN_EMBEDDED={}", dst.display());
}

fn stage_with_optional_override(
    workspace: &Path,
    target_dir: &Path,
    out_dir: &Path,
    pkg: &str,
    override_env: &str,
    embed_env: &str,
) {
    if std::env::var_os(override_env).is_some_and(|v| !v.is_empty()) {
        let empty = out_dir.join(format!("{pkg}.empty"));
        std::fs::write(&empty, []).expect("write empty placeholder");
        println!("cargo:rustc-env={embed_env}={}", empty.display());
        println!(
            "cargo:warning=lns-service build.rs: {override_env} is set; \
             skipping {pkg} cross-build (override will be read at runtime)."
        );
        return;
    }
    let dst = cross_build_and_stage(workspace, target_dir, out_dir, pkg);
    println!("cargo:rustc-env={embed_env}={}", dst.display());
}

fn cross_build_and_stage(
    workspace: &Path,
    target_dir: &Path,
    out_dir: &Path,
    pkg: &str,
) -> PathBuf {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(&cargo);
    cmd.args(["build", "-p", pkg, "--profile", PROFILE, "--target", TARGET])
        .current_dir(workspace);

    if std::env::var_os("CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER").is_none() {
        cmd.env("CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER", "rust-lld");
    }

    cmd.env_remove("RUSTFLAGS");

    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("invoking cargo to cross-build {pkg} failed to start: {e}"));
    if !status.success() {
        panic!(
            "lns-service's build.rs failed to cross-build {pkg} for {TARGET}. \
             Check that the target is installed:\n\
             \n\
             \trustup target add {TARGET}\n\
             \n\
             Or set LNS_INIT_BIN=/path/to/pre-built/lns-init to bypass the cross-build."
        );
    }

    let elf = target_dir.join(TARGET).join(PROFILE).join(pkg);
    if !elf.is_file() {
        panic!(
            "{pkg} build succeeded but artifact not found at {} — \
             unexpected CARGO_TARGET_DIR layout?",
            elf.display()
        );
    }

    let dst = out_dir.join(pkg);
    std::fs::copy(&elf, &dst).unwrap_or_else(|e| {
        panic!(
            "copying {pkg} from {} to {}: {e}",
            elf.display(),
            dst.display()
        )
    });

    const MIN_EMBED_BYTES: u64 = 10_000;
    let dst_size = std::fs::metadata(&dst)
        .unwrap_or_else(|e| panic!("stat'ing {}: {e}", dst.display()))
        .len();
    if dst_size < MIN_EMBED_BYTES {
        panic!(
            "{pkg} artifact at {} is suspiciously small ({} bytes; \
             expected > {}). Real static-musl Rust is multi-hundred KB. \
             Refusing to embed — see build.rs for the size-floor rationale.",
            dst.display(),
            dst_size,
            MIN_EMBED_BYTES,
        );
    }

    dst
}

fn emit_kernel_pin(manifest_path: &Path) {
    println!("cargo:rerun-if-changed={}", manifest_path.display());

    let raw = std::fs::read_to_string(manifest_path).unwrap_or_else(|e| {
        panic!(
            "reading kernel manifest at {}: {e}\n\
             This file is the source of truth for the guest kernel pin; \
             see runbooks/kernel-bump.md.",
            manifest_path.display()
        )
    });

    let manifest: KernelManifest = toml::from_str(&raw).unwrap_or_else(|e| {
        panic!(
            "parsing {}: {e}\n\
             Expected schema: see comments at the top of kernels.toml.",
            manifest_path.display()
        )
    });
    let cur = &manifest.current;

    let expected_published = cur
        .kernel_filename
        .strip_prefix("vmlinuz-")
        .unwrap_or(&cur.kernel_filename);
    if expected_published != cur.published_version {
        panic!(
            "kernels.toml: published_version=\"{}\" doesn't match \
             strip_prefix(kernel_filename, \"vmlinuz-\")=\"{}\". \
             Fix one or the other; CI enforces the same invariant.",
            cur.published_version, expected_published,
        );
    }

    let target_arch =
        std::env::var("CARGO_CFG_TARGET_ARCH").expect("CARGO_CFG_TARGET_ARCH set by cargo");
    let sha = cur.sha256.get(&target_arch).unwrap_or_else(|| {
        let arches: Vec<_> = cur.sha256.keys().cloned().collect();
        panic!(
            "kernels.toml has no [current.sha256] entry for target_arch=\"{}\". \
             Declared arches: {:?}. Either lns-cli doesn't support this target, \
             or the bump-kernel workflow hasn't completed for it.",
            target_arch, arches,
        )
    });
    if sha.is_empty() {
        panic!(
            "kernels.toml: [current.sha256].{} is empty. This commit was \
             likely opened by the bump-kernel CLI but the CI publish-kernel \
             workflow hasn't filled it in yet. Wait for the bot back-fill \
             commit, then re-run CI.",
            target_arch,
        );
    }
    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        panic!(
            "kernels.toml: [current.sha256].{} is not a 64-char hex string \
             (got len={}: {:?}).",
            target_arch,
            sha.len(),
            sha,
        );
    }

    println!("cargo:rustc-env=KERNEL_VERSION={}", cur.published_version);
    println!("cargo:rustc-env=KERNEL_SHA256={}", sha);
}
