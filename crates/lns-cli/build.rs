use std::process::Command;

fn main() {
    emit_build_sha();
}

fn emit_build_sha() {
    let mut watched = vec!["HEAD".to_string(), "index".to_string()];
    // A commit moves the branch ref HEAD points at, not the HEAD file itself.
    watched.extend(git_head_ref());
    for name in watched {
        if let Some(path) = git_path(&name) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let stamp = match git_short_sha() {
        Some(sha) if git_is_dirty() => format!("{sha}-dirty"),
        Some(sha) => sha,
        None => "unknown".to_string(),
    };
    println!("cargo:rustc-env=LNS_BUILD_SHA={stamp}");
}

fn git_head_ref() -> Option<String> {
    let out = Command::new("git")
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let head_ref = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!head_ref.is_empty()).then_some(head_ref)
}

fn git_short_sha() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

fn git_is_dirty() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map(|out| out.status.success() && !out.stdout.is_empty())
        .unwrap_or(false)
}

fn git_path(name: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--git-path", name])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        return None;
    }
    // Absolute so Cargo watches the real file regardless of how it resolves rerun-if-changed.
    std::fs::canonicalize(&path)
        .ok()
        .map(|p| p.display().to_string())
}
