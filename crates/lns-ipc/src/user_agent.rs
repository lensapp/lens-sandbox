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
    RegistryPull,
    RegistryPush,
    RegistryLogin,
    ConnectorOauth,
    ToolIndex,
    AssetDownload,
}

impl Method {
    /// Declaration order, which is also the slot order [`Identity`] indexes by.
    pub const ALL: [Method; 10] = [
        Method::InstallScript,
        Method::CliUpdate,
        Method::CliLogin,
        Method::ServiceUpdateCheck,
        Method::RegistryPull,
        Method::RegistryPush,
        Method::RegistryLogin,
        Method::ConnectorOauth,
        Method::ToolIndex,
        Method::AssetDownload,
    ];

    pub fn product(self) -> &'static str {
        match self {
            Method::InstallScript => "lns-install",
            _ => "lns",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Method::InstallScript => "install-script",
            Method::CliUpdate => "cli-update",
            Method::CliLogin => "cli-login",
            Method::ServiceUpdateCheck => "service-update-check",
            Method::RegistryPull => "registry-pull",
            Method::RegistryPush => "registry-push",
            Method::RegistryLogin => "registry-login",
            Method::ConnectorOauth => "connector-oauth",
            Method::ToolIndex => "tool-index",
            Method::AssetDownload => "asset-download",
        }
    }
}

/// One process's outbound identity: every header it can send, built once so a caller needing `&'static str` never leaks a fresh one per request.
pub struct Identity {
    headers: [String; Method::ALL.len()],
}

impl Identity {
    pub fn new(version: &str, platform: &PlatformInfo) -> Self {
        Self {
            headers: Method::ALL.map(|method| user_agent(version, platform, method)),
        }
    }

    /// Borrowed for `'static` because `oci_client::client::ClientConfig::user_agent` accepts nothing shorter.
    pub fn header(&'static self, method: Method) -> &'static str {
        &self.headers[method as usize]
    }
}

const MAX_FIELD_LEN: usize = 64;

/// `$SHELL` is whatever the user exported and a uname field is whatever the kernel reports, so a field is folded to a bounded, header-legal token before it reaches a `User-Agent`: a control character makes the header invalid outright, and the format's own separators would otherwise forge a field.
fn field(raw: &str) -> String {
    let safe: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+' | '/' | ':') {
                c
            } else {
                '_'
            }
        })
        .take(MAX_FIELD_LEN)
        .collect();
    if safe.is_empty() {
        "unknown".to_string()
    } else {
        safe
    }
}

pub fn user_agent(version: &str, p: &PlatformInfo, method: Method) -> String {
    format!(
        "{product}/{version} (os={os}; arch={arch}; kernel={os}/{rel}; shell={shell}; method={method})",
        product = method.product(),
        version = field(version),
        os = field(&p.os),
        arch = field(&p.arch),
        rel = field(&p.kernel_release),
        shell = field(&p.shell),
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

    fn is_header_safe(ua: &str) -> bool {
        ua.bytes().all(|b| b >= 32 && b != 127)
    }

    #[test]
    fn a_control_character_in_the_shell_name_cannot_produce_an_invalid_header() {
        let ua = user_agent(
            "0.21.0",
            &platform("Darwin", "arm64", "24.6.0", "z\nsh\r"),
            Method::RegistryPull,
        );
        assert!(is_header_safe(&ua), "{ua:?}");
        assert!(ua.contains("shell=z_sh_;"), "{ua}");
    }

    #[test]
    fn a_field_that_mimics_the_format_cannot_forge_another_one() {
        let ua = user_agent(
            "0.21.0",
            &platform("Darwin", "arm64", "24.6.0", "zsh; method=install-script"),
            Method::RegistryPull,
        );
        assert!(ua.ends_with("method=registry-pull)"), "{ua}");
        assert!(!ua.contains("method=install-script"), "{ua}");
    }

    #[test]
    fn a_hostile_length_field_is_truncated() {
        let ua = user_agent(
            "0.21.0",
            &platform("Darwin", "arm64", "24.6.0", &"z".repeat(500)),
            Method::RegistryPull,
        );
        assert!(ua.contains(&format!("shell={};", "z".repeat(64))), "{ua}");
    }

    #[test]
    fn a_field_left_empty_by_sanitising_says_unknown() {
        let ua = user_agent(
            "",
            &platform("Darwin", "arm64", "", "\u{1}"),
            Method::RegistryPull,
        );
        assert!(is_header_safe(&ua), "{ua:?}");
        assert!(ua.starts_with("lns/unknown ("), "{ua}");
        assert!(ua.contains("kernel=Darwin/unknown;"), "{ua}");
        assert!(ua.contains("shell=_;"), "{ua}");
    }

    #[test]
    fn every_method_carries_its_own_token_under_one_product() {
        let p = platform("Darwin", "arm64", "24.6.0", "zsh");
        let tokens: Vec<&str> = Method::ALL.iter().map(|m| m.as_str()).collect();
        let mut unique = tokens.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            tokens.len(),
            "duplicate method token: {tokens:?}"
        );
        for method in Method::ALL {
            let ua = user_agent("1.2.3", &p, method);
            assert!(
                ua.ends_with(&format!("method={})", method.as_str())),
                "{ua}"
            );
            assert!(
                ua.starts_with(&format!("{}/1.2.3 (", method.product())),
                "{ua}"
            );
        }
    }

    #[test]
    fn only_the_install_script_speaks_under_its_own_product() {
        for method in Method::ALL {
            let expected = if method == Method::InstallScript {
                "lns-install"
            } else {
                "lns"
            };
            assert_eq!(method.product(), expected, "{:?}", method.as_str());
        }
    }

    #[test]
    fn registry_traffic_names_the_verb_it_is_performing() {
        assert_eq!(Method::RegistryPull.as_str(), "registry-pull");
        assert_eq!(Method::RegistryPush.as_str(), "registry-push");
        assert_eq!(Method::RegistryLogin.as_str(), "registry-login");
    }

    #[test]
    fn the_remaining_outbound_callers_name_themselves_too() {
        assert_eq!(Method::ConnectorOauth.as_str(), "connector-oauth");
        assert_eq!(Method::ToolIndex.as_str(), "tool-index");
        assert_eq!(Method::AssetDownload.as_str(), "asset-download");
    }

    #[test]
    fn all_lists_every_method_at_the_slot_identity_indexes_it_by() {
        for (slot, method) in Method::ALL.iter().enumerate() {
            assert_eq!(*method as usize, slot, "{}", method.as_str());
        }
    }

    #[test]
    fn an_identity_hands_out_one_header_per_method() {
        static ID: std::sync::OnceLock<Identity> = std::sync::OnceLock::new();
        let id = ID
            .get_or_init(|| Identity::new("0.16.0", &platform("Linux", "x86_64", "6.6.0", "bash")));
        assert_eq!(
            id.header(Method::RegistryPull),
            "lns/0.16.0 (os=Linux; arch=x86_64; kernel=Linux/6.6.0; shell=bash; method=registry-pull)"
        );
        assert_eq!(
            id.header(Method::InstallScript),
            "lns-install/0.16.0 (os=Linux; arch=x86_64; kernel=Linux/6.6.0; shell=bash; method=install-script)"
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
