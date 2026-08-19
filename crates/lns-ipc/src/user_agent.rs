use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct PlatformInfo {
    pub os: String,
    pub arch: String,
    pub kernel_release: String,
    pub shell: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    InstallScript,
    CliUpdate,
    CliLogin,
    ServiceUpdateCheck,
}

impl Method {
    pub fn product(self) -> &'static str {
        match self {
            Method::InstallScript => "lns-install",
            Method::CliUpdate | Method::CliLogin | Method::ServiceUpdateCheck => "lns",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Method::InstallScript => "install-script",
            Method::CliUpdate => "cli-update",
            Method::CliLogin => "cli-login",
            Method::ServiceUpdateCheck => "service-update-check",
        }
    }
}

pub fn user_agent(version: &str, p: &PlatformInfo, method: Method) -> String {
    format!(
        "{product}/{version} (os={os}; arch={arch}; kernel={os}/{rel}; shell={shell}; method={method})",
        product = method.product(),
        os = p.os,
        arch = p.arch,
        rel = p.kernel_release,
        shell = p.shell,
        method = method.as_str(),
    )
}

pub trait Uname {
    fn uname(&self) -> Option<(String, String, String)>;
}

pub fn env_os_to_uname_sysname(env_os: &str) -> Option<&'static str> {
    match env_os {
        "macos" => Some("Darwin"),
        "linux" => Some("Linux"),
        _ => None,
    }
}

pub fn shell_basename_from(shell: Option<OsString>) -> String {
    shell
        .and_then(|v| {
            PathBuf::from(v)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn uname_fields_with(
    uname: &dyn Uname,
    env_os: &str,
    env_arch: &str,
) -> (String, String, String) {
    match uname.uname() {
        Some(fields) => fields,
        None => {
            let sysname = env_os_to_uname_sysname(env_os).unwrap_or(env_os);
            (sysname.to_string(), env_arch.to_string(), String::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform(os: &str, arch: &str, kernel_release: &str, shell: &str) -> PlatformInfo {
        PlatformInfo {
            os: os.into(),
            arch: arch.into(),
            kernel_release: kernel_release.into(),
            shell: shell.into(),
        }
    }

    #[test]
    fn install_script_method_matches_the_shell_installer_shape() {
        let ua = user_agent(
            "0.4.0",
            &platform("Darwin", "arm64", "24.6.0", "zsh"),
            Method::InstallScript,
        );
        assert_eq!(
            ua,
            "lns-install/0.4.0 (os=Darwin; arch=arm64; kernel=Darwin/24.6.0; shell=zsh; method=install-script)"
        );
    }

    #[test]
    fn cli_update_method_uses_the_lns_token() {
        let ua = user_agent(
            "0.3.0",
            &platform("Darwin", "arm64", "24.6.0", "zsh"),
            Method::CliUpdate,
        );
        assert_eq!(
            ua,
            "lns/0.3.0 (os=Darwin; arch=arm64; kernel=Darwin/24.6.0; shell=zsh; method=cli-update)"
        );
    }

    #[test]
    fn service_update_check_method_shares_the_cli_shape_and_token() {
        let ua = user_agent(
            "9.9.9",
            &platform("Linux", "x86_64", "6.6.0-test", "unknown"),
            Method::ServiceUpdateCheck,
        );
        assert_eq!(
            ua,
            "lns/9.9.9 (os=Linux; arch=x86_64; kernel=Linux/6.6.0-test; shell=unknown; method=service-update-check)"
        );
    }

    #[test]
    fn cli_login_method_shares_the_cli_shape_and_token() {
        let ua = user_agent(
            "0.16.0",
            &platform("Darwin", "arm64", "24.6.0", "zsh"),
            Method::CliLogin,
        );
        assert_eq!(
            ua,
            "lns/0.16.0 (os=Darwin; arch=arm64; kernel=Darwin/24.6.0; shell=zsh; method=cli-login)"
        );
    }

    #[test]
    fn env_os_to_uname_sysname_maps_known_targets() {
        assert_eq!(env_os_to_uname_sysname("macos"), Some("Darwin"));
        assert_eq!(env_os_to_uname_sysname("linux"), Some("Linux"));
        assert_eq!(env_os_to_uname_sysname("freebsd"), None);
        assert_eq!(env_os_to_uname_sysname(""), None);
    }

    #[test]
    fn shell_basename_from_extracts_the_basename() {
        assert_eq!(shell_basename_from(Some("/bin/zsh".into())), "zsh");
        assert_eq!(
            shell_basename_from(Some("/usr/local/bin/fish".into())),
            "fish"
        );
    }

    #[test]
    fn shell_basename_from_is_unknown_when_shell_is_absent() {
        assert_eq!(shell_basename_from(None), "unknown");
    }

    #[test]
    fn shell_basename_from_is_unknown_when_path_has_no_file_name() {
        assert_eq!(shell_basename_from(Some("/".into())), "unknown");
    }

    struct FakeUname {
        result: Option<(String, String, String)>,
    }

    impl Uname for FakeUname {
        fn uname(&self) -> Option<(String, String, String)> {
            self.result.clone()
        }
    }

    #[test]
    fn uname_fields_with_success_returns_all_three_fields() {
        let fake = FakeUname {
            result: Some(("Linux".into(), "x86_64".into(), "6.1.0".into())),
        };
        assert_eq!(
            uname_fields_with(&fake, "linux", "x86_64"),
            ("Linux".into(), "x86_64".into(), "6.1.0".into())
        );
    }

    #[test]
    fn uname_fields_with_failure_maps_known_env_os_and_empties_release() {
        let fake = FakeUname { result: None };
        assert_eq!(
            uname_fields_with(&fake, "linux", "x86_64"),
            ("Linux".into(), "x86_64".into(), String::new())
        );
    }

    #[test]
    fn uname_fields_with_failure_passes_through_unmapped_env_os() {
        let fake = FakeUname { result: None };
        assert_eq!(
            uname_fields_with(&fake, "solaris", "sparc"),
            ("solaris".into(), "sparc".into(), String::new())
        );
    }
}
