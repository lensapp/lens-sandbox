use std::collections::HashSet;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// Selects the built-in resolver that turns a host into this capability's parts; the resolution is code, not data, so the catalog names it rather than describing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Provides {
    GitSigning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Locate {
    Command(Vec<String>),
    Env(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SocketSpec {
    pub locate: Locate,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostAccess {
    pub id: String,
    pub name: String,
    pub provides: Provides,
    pub openpgp_socket: SocketSpec,
    pub ssh_socket: SocketSpec,
    pub gnupg_home: String,
    pub git_config: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostAccessCatalog {
    #[serde(default)]
    pub host_access: Vec<HostAccess>,
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A guest target is absolute or home-relative; `..` would let a catalog entry write outside the workload's home.
fn valid_target(target: &str) -> bool {
    let rest = match target
        .strip_prefix("~/")
        .or_else(|| target.strip_prefix('/'))
    {
        Some(rest) => rest,
        None => return false,
    };
    !rest.is_empty()
        && !rest.split('/').any(|seg| seg == "..")
        && !rest.chars().any(char::is_control)
}

impl SocketSpec {
    fn validate(&self, id: &str, which: &str) -> Result<(), String> {
        match &self.locate {
            Locate::Command(argv) if argv.is_empty() => {
                return Err(format!(
                    "host access {id:?}: {which} locate command is empty"
                ));
            }
            Locate::Env(name) if name.is_empty() => {
                return Err(format!("host access {id:?}: {which} locate env is empty"));
            }
            _ => {}
        }
        if !valid_target(&self.target) {
            return Err(format!(
                "host access {id:?}: {which} target {:?} must be absolute or start with ~/, contain no .. segment, and carry no control character",
                self.target
            ));
        }
        Ok(())
    }
}

impl HostAccess {
    pub fn validate(&self) -> Result<(), String> {
        if !is_valid_id(&self.id) {
            return Err(format!(
                "host access id {:?} must be a lowercase kebab-case name",
                self.id
            ));
        }
        self.openpgp_socket.validate(&self.id, "openpgpSocket")?;
        self.ssh_socket.validate(&self.id, "sshSocket")?;
        for (field, value) in [
            ("gnupgHome", &self.gnupg_home),
            ("gitConfig", &self.git_config),
        ] {
            if !valid_target(value) {
                return Err(format!(
                    "host access {:?}: {field} {value:?} must be absolute or start with ~/, contain no .. segment, and carry no control character",
                    self.id
                ));
            }
        }
        Ok(())
    }
}

impl HostAccessCatalog {
    pub fn validate(&self) -> Result<(), String> {
        let mut seen: HashSet<&str> = HashSet::new();
        for entry in &self.host_access {
            entry.validate()?;
            if !seen.insert(entry.id.as_str()) {
                return Err(format!("duplicate host access id {:?}", entry.id));
            }
        }
        Ok(())
    }
}

/// Panics on a malformed manifest; the shipped catalog is test-proven well-formed, so the production caller never hits that arm.
static BUNDLED: LazyLock<Vec<HostAccess>> = LazyLock::new(|| {
    let catalog: HostAccessCatalog = serde_yaml::from_str(include_str!("host_access.yaml"))
        .expect("bundled host access catalog must be valid YAML");
    catalog
        .validate()
        .expect("bundled host access catalog must be internally consistent");
    catalog.host_access
});

pub fn bundled_host_access() -> &'static [HostAccess] {
    BUNDLED.as_slice()
}

pub fn find(id: &str) -> Option<&'static HostAccess> {
    bundled_host_access().iter().find(|e| e.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> HostAccess {
        HostAccess {
            id: "some-access".into(),
            name: "Some access".into(),
            provides: Provides::GitSigning,
            openpgp_socket: SocketSpec {
                locate: Locate::Command(vec!["some-locate".into(), "--dir".into()]),
                target: "~/.gnupg/S.gpg-agent".into(),
            },
            ssh_socket: SocketSpec {
                locate: Locate::Env("SOME_AUTH_SOCK".into()),
                target: "~/.ssh/lns-agent.sock".into(),
            },
            gnupg_home: "~/.gnupg".into(),
            git_config: "~/.gitconfig".into(),
        }
    }

    #[test]
    fn the_bundled_catalog_ships_git_signing_and_is_internally_consistent() {
        let ids: Vec<&str> = bundled_host_access()
            .iter()
            .map(|e| e.id.as_str())
            .collect();
        assert!(ids.contains(&"git-signing"), "got {ids:?}");
    }

    #[test]
    fn find_resolves_a_bundled_id_and_rejects_an_unknown_one() {
        assert_eq!(
            find("git-signing").map(|e| e.id.as_str()),
            Some("git-signing")
        );
        assert!(find("not-in-catalog").is_none());
    }

    #[test]
    fn a_well_formed_entry_validates() {
        entry().validate().expect("should validate");
    }

    #[test]
    fn an_id_that_is_not_a_lowercase_kebab_name_is_refused() {
        for bad in ["Some_Access", "some access", "", "-lead", "trail-"] {
            let mut e = entry();
            e.id = bad.into();
            assert!(e.validate().is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn a_guest_target_must_be_absolute_or_home_relative_with_no_parent_segments() {
        for bad in [
            "relative/path",
            "~/../escape",
            "~",
            "",
            "~/ok/../bad",
            "~/bad\nsock",
            "~/bad\tsock",
        ] {
            let mut e = entry();
            e.openpgp_socket.target = bad.into();
            assert!(e.validate().is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn a_bad_gnupg_home_or_git_config_target_is_refused_and_the_field_is_named() {
        let mut home = entry();
        home.gnupg_home = "relative/gnupg".into();
        let err = home.validate().unwrap_err();
        assert!(err.contains("gnupgHome"), "got: {err}");

        let mut config = entry();
        config.git_config = "~/../escape".into();
        let err = config.validate().unwrap_err();
        assert!(err.contains("gitConfig"), "got: {err}");
    }

    #[test]
    fn an_absolute_guest_target_is_accepted() {
        let mut e = entry();
        e.openpgp_socket.target = "/run/lns/agent.sock".into();
        e.validate().expect("absolute target is fine");
    }

    #[test]
    fn an_empty_locate_command_is_refused() {
        let mut e = entry();
        e.openpgp_socket.locate = Locate::Command(Vec::new());
        assert!(e.validate().is_err());
    }

    #[test]
    fn an_empty_locate_env_name_is_refused() {
        let mut e = entry();
        e.ssh_socket.locate = Locate::Env(String::new());
        assert!(e.validate().is_err());
    }

    #[test]
    fn a_catalog_with_duplicate_ids_is_refused() {
        let catalog = HostAccessCatalog {
            host_access: vec![entry(), entry()],
        };
        assert!(catalog.validate().is_err());
    }

    #[test]
    fn a_catalog_of_distinct_valid_entries_validates() {
        let mut second = entry();
        second.id = "other-access".into();
        let catalog = HostAccessCatalog {
            host_access: vec![entry(), second],
        };
        catalog.validate().expect("should validate");
    }

    #[test]
    fn the_catalog_yaml_round_trips_losslessly() {
        let catalog = HostAccessCatalog {
            host_access: vec![entry()],
        };
        let yaml = serde_yaml::to_string(&catalog).unwrap();
        let parsed: HostAccessCatalog = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, catalog);
    }

    #[test]
    fn the_bundled_git_signing_entry_pins_what_it_ships() {
        let e = find("git-signing").expect("bundled");
        assert_eq!(e.provides, Provides::GitSigning);
        assert_eq!(
            e.openpgp_socket.locate,
            Locate::Command(vec![
                "gpgconf".into(),
                "--list-dir".into(),
                "agent-extra-socket".into(),
            ])
        );
        assert_eq!(e.openpgp_socket.target, "~/.gnupg/S.gpg-agent");
        assert_eq!(e.ssh_socket.locate, Locate::Env("SSH_AUTH_SOCK".into()));
        assert_eq!(e.gnupg_home, "~/.gnupg");
        assert_eq!(e.git_config, "~/.gitconfig");
    }
}
