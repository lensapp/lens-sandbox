#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Result, bail};

#[derive(Debug)]
pub(crate) struct VmmBinaries {
    pub cloud_hypervisor: PathBuf,
    pub virtiofsd: PathBuf,
}

pub(crate) fn resolve(env_get: impl Fn(&str) -> Option<OsString>) -> Result<VmmBinaries> {
    Ok(VmmBinaries {
        cloud_hypervisor: resolve_one(&env_get, "LNS_CLOUD_HYPERVISOR_BIN", "cloud-hypervisor")?,
        virtiofsd: resolve_one(&env_get, "LNS_VIRTIOFSD_BIN", "virtiofsd")?,
    })
}

fn resolve_one(
    env_get: &impl Fn(&str) -> Option<OsString>,
    var: &str,
    name: &str,
) -> Result<PathBuf> {
    match env_get(var) {
        Some(value) => {
            let path = PathBuf::from(value);
            if !path.is_file() {
                bail!("{var}={} is not a regular file", path.display());
            }
            Ok(path)
        }
        None => bail!(
            "{name} binary not found. The CDN-published, sha-pinned {name} is not \
             wired yet; set {var}=/path/to/{name} to point lns at a local build."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_one_accepts_an_existing_file_from_the_override() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path().to_path_buf();
        let env = |k: &str| (k == "LNS_X").then(|| path.clone().into_os_string());
        let resolved = resolve_one(&env, "LNS_X", "thing").unwrap();
        assert_eq!(resolved, path);
    }

    #[test]
    fn resolve_one_rejects_a_missing_override_target() {
        let env = |k: &str| (k == "LNS_X").then(|| OsString::from("/no/such/binary"));
        let err = resolve_one(&env, "LNS_X", "thing").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("LNS_X=/no/such/binary"), "got {msg}");
        assert!(msg.contains("not a regular file"), "got {msg}");
    }

    #[test]
    fn resolve_one_without_override_names_the_env_var_to_set() {
        let env = |_: &str| None;
        let err = resolve_one(&env, "LNS_VIRTIOFSD_BIN", "virtiofsd").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("virtiofsd binary not found"), "got {msg}");
        assert!(
            msg.contains("LNS_VIRTIOFSD_BIN=/path/to/virtiofsd"),
            "got {msg}"
        );
    }

    #[test]
    fn resolve_reports_the_first_missing_binary() {
        let ch = tempfile::NamedTempFile::new().unwrap();
        let ch_path = ch.path().to_path_buf();
        let env = move |k: &str| match k {
            "LNS_CLOUD_HYPERVISOR_BIN" => Some(ch_path.clone().into_os_string()),
            _ => None,
        };
        let err = resolve(env).unwrap_err();
        assert!(
            format!("{err:#}").contains("virtiofsd binary not found"),
            "got {err:#}"
        );
    }
}
