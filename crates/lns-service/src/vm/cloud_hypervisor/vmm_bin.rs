#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

#[derive(Debug)]
pub(crate) struct VmmBinaries {
    pub cloud_hypervisor: PathBuf,
    pub virtiofsd: PathBuf,
}

pub(crate) fn resolve(env_get: impl Fn(&str) -> Option<OsString>) -> Result<VmmBinaries> {
    Ok(VmmBinaries {
        cloud_hypervisor: resolve_one(
            &env_get,
            "LNS_CLOUD_HYPERVISOR_BIN",
            "cloud-hypervisor",
            &[],
        )?,
        virtiofsd: resolve_one(
            &env_get,
            "LNS_VIRTIOFSD_BIN",
            "virtiofsd",
            &virtiofsd_fallback_dirs(),
        )?,
    })
}

/// Distros ship the Rust virtiofsd off PATH (Debian/Ubuntu under /usr/libexec); search these so a stock apt install needs no override.
fn virtiofsd_fallback_dirs() -> Vec<PathBuf> {
    ["/usr/libexec", "/usr/lib/virtiofsd", "/usr/lib/qemu"]
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

fn resolve_one(
    env_get: &impl Fn(&str) -> Option<OsString>,
    var: &str,
    name: &str,
    fallback_dirs: &[PathBuf],
) -> Result<PathBuf> {
    if let Some(value) = env_get(var) {
        let path = PathBuf::from(value);
        if !path.is_file() {
            bail!("{var}={} is not a regular file", path.display());
        }
        return Ok(path);
    }
    if let Some(found) = which(name, env_get) {
        return Ok(found);
    }
    for dir in fallback_dirs {
        let cand = dir.join(name);
        if is_executable_file(&cand) {
            return Ok(cand);
        }
    }
    bail!(
        "{name} was not found on PATH{extra}. lns drives an out-of-process {name} on Linux; \
         install it (e.g. your distro package or a static build) or set \
         {var}=/path/to/{name}. A CDN-bundled, sha-pinned {name} is the planned default.",
        extra = fallback_dirs_phrase(fallback_dirs)
    )
}

fn fallback_dirs_phrase(fallback_dirs: &[PathBuf]) -> String {
    if fallback_dirs.is_empty() {
        return String::new();
    }
    let joined = fallback_dirs
        .iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!(" or in {joined}")
}

fn which(name: &str, env_get: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let path_var = env_get("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|cand| is_executable_file(cand))
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn executable(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[test]
    fn override_is_used_without_consulting_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let pinned = executable(dir.path(), "pinned-ch");
        let pinned_for_env = pinned.clone();
        // No PATH entry: resolve_one must return the override without falling back.
        let env = move |k: &str| (k == "LNS_X").then(|| pinned_for_env.clone().into_os_string());
        assert_eq!(resolve_one(&env, "LNS_X", "thing", &[]).unwrap(), pinned);
    }

    #[test]
    fn rejects_a_missing_override_target() {
        let env = |k: &str| (k == "LNS_X").then(|| OsString::from("/no/such/binary"));
        let err = resolve_one(&env, "LNS_X", "thing", &[]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("LNS_X=/no/such/binary"), "got {msg}");
        assert!(msg.contains("not a regular file"), "got {msg}");
    }

    #[test]
    fn falls_back_to_an_executable_on_path() {
        let path_dir = tempfile::TempDir::new().unwrap();
        let ch = executable(path_dir.path(), "cloud-hypervisor");
        let env = |k: &str| (k == "PATH").then(|| path_dir.path().as_os_str().to_os_string());
        assert_eq!(
            resolve_one(&env, "LNS_CLOUD_HYPERVISOR_BIN", "cloud-hypervisor", &[]).unwrap(),
            ch
        );
    }

    #[test]
    fn finds_virtiofsd_in_a_well_known_dir_off_path() {
        let libexec = tempfile::TempDir::new().unwrap();
        let vfsd = executable(libexec.path(), "virtiofsd");
        // PATH has no virtiofsd; the off-PATH fallback dir supplies it (the apt /usr/libexec case).
        let empty = tempfile::TempDir::new().unwrap();
        let env = move |k: &str| (k == "PATH").then(|| empty.path().as_os_str().to_os_string());
        let dirs = vec![libexec.path().to_path_buf()];
        assert_eq!(
            resolve_one(&env, "LNS_VIRTIOFSD_BIN", "virtiofsd", &dirs).unwrap(),
            vfsd
        );
    }

    #[test]
    fn path_wins_over_a_well_known_dir() {
        let path_dir = tempfile::TempDir::new().unwrap();
        let on_path = executable(path_dir.path(), "virtiofsd");
        let libexec = tempfile::TempDir::new().unwrap();
        let _shadowed = executable(libexec.path(), "virtiofsd");
        let env = move |k: &str| (k == "PATH").then(|| path_dir.path().as_os_str().to_os_string());
        let dirs = vec![libexec.path().to_path_buf()];
        assert_eq!(
            resolve_one(&env, "LNS_VIRTIOFSD_BIN", "virtiofsd", &dirs).unwrap(),
            on_path
        );
    }

    #[test]
    fn skips_a_non_executable_path_entry() {
        let path_dir = tempfile::TempDir::new().unwrap();
        std::fs::write(path_dir.path().join("virtiofsd"), b"not executable").unwrap();
        let env = |k: &str| (k == "PATH").then(|| path_dir.path().as_os_str().to_os_string());
        let err = resolve_one(&env, "LNS_VIRTIOFSD_BIN", "virtiofsd", &[]).unwrap_err();
        assert!(
            format!("{err:#}").contains("was not found on PATH"),
            "a non-executable file must not satisfy the lookup"
        );
    }

    #[test]
    fn without_override_or_path_names_the_env_var_and_is_actionable() {
        let env = |_: &str| None;
        let dirs = vec![PathBuf::from("/usr/libexec")];
        let err = resolve_one(&env, "LNS_VIRTIOFSD_BIN", "virtiofsd", &dirs).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("virtiofsd was not found on PATH"), "got {msg}");
        assert!(
            msg.contains("/usr/libexec"),
            "names the searched dir: {msg}"
        );
        assert!(
            msg.contains("LNS_VIRTIOFSD_BIN=/path/to/virtiofsd"),
            "got {msg}"
        );
    }

    #[test]
    fn cloud_hypervisor_error_omits_the_fallback_phrase() {
        let env = |_: &str| None;
        let err =
            resolve_one(&env, "LNS_CLOUD_HYPERVISOR_BIN", "cloud-hypervisor", &[]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("cloud-hypervisor was not found on PATH."),
            "got {msg}"
        );
        assert!(
            !msg.contains(" or in "),
            "no fallback dirs → no fallback phrase: {msg}"
        );
    }

    #[test]
    fn virtiofsd_fallback_dirs_include_the_debian_libexec_path() {
        assert!(
            virtiofsd_fallback_dirs().contains(&PathBuf::from("/usr/libexec")),
            "the apt package lands virtiofsd in /usr/libexec"
        );
    }

    #[test]
    fn resolve_reports_the_first_unresolved_binary() {
        let dir = tempfile::TempDir::new().unwrap();
        let ch = executable(dir.path(), "cloud-hypervisor");
        let env = move |k: &str| match k {
            "LNS_CLOUD_HYPERVISOR_BIN" => Some(ch.clone().into_os_string()),
            _ => None,
        };
        let err = resolve(env).unwrap_err();
        assert!(
            format!("{err:#}").contains("virtiofsd was not found"),
            "got {err:#}"
        );
    }
}
