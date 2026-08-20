//! Resolve a script's `USER[:GROUP]` the way `lns-init` resolves the workload's, rather than through `SandboxCredentials::resolve`, which needs a same-named group and so answers nothing for `user: 1000`.

use lens_sandbox_core::privilege::SandboxCredentials;

mod real;
pub(crate) use real::GuestPasswd;

/// The guest's own name-to-id tables, injected so every branch is reachable without a real passwd.
pub(crate) trait Passwd: Send + Sync {
    fn uid_of(&self, name: &str) -> Option<u32>;
    fn primary_gid_of(&self, name: &str) -> Option<u32>;
    fn gid_of_group(&self, group: &str) -> Option<u32>;
}

/// Resolve one `USER[:GROUP]`; an unresolvable name is an error rather than a fallback, because falling back would run a script as an identity nobody named.
pub(crate) fn resolve(spec: &str, passwd: &dyn Passwd) -> Result<SandboxCredentials, String> {
    let (name, group) = split(spec)?;
    let uid = passwd
        .uid_of(name)
        .or_else(|| name.parse::<u32>().ok())
        .ok_or_else(|| format!("no user {name:?} in the guest's passwd"))?;
    let gid = match group {
        // Number first, as lns-init resolves the workload's group: §3.1.3 uses a number as the id directly, so a group *named* for a numeral must not outrank it.
        Some(group) => group
            .parse::<u32>()
            .ok()
            .or_else(|| passwd.gid_of_group(group))
            .ok_or_else(|| format!("no group {group:?} in the guest's group file"))?,
        None => passwd.primary_gid_of(name).unwrap_or(uid),
    };
    SandboxCredentials::resolve_by_uid(uid, gid)
}

fn split(spec: &str) -> Result<(&str, Option<&str>), String> {
    let mut parts = spec.split(':');
    let name = parts.next().unwrap_or_default();
    let group = parts.next();
    if parts.next().is_some() {
        return Err(format!(
            "invalid user {spec:?}: expected USER or USER:GROUP"
        ));
    }
    if name.is_empty() || group.is_some_and(str::is_empty) {
        return Err(format!("invalid user {spec:?}: no segment may be empty"));
    }
    Ok((name, group))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakePasswd {
        users: Vec<(&'static str, u32, u32)>,
        groups: Vec<(&'static str, u32)>,
    }

    impl Passwd for FakePasswd {
        fn uid_of(&self, name: &str) -> Option<u32> {
            self.users
                .iter()
                .find(|(n, ..)| *n == name)
                .map(|(_, uid, _)| *uid)
        }
        fn primary_gid_of(&self, name: &str) -> Option<u32> {
            self.users
                .iter()
                .find(|(n, ..)| *n == name)
                .map(|(.., gid)| *gid)
        }
        fn gid_of_group(&self, group: &str) -> Option<u32> {
            self.groups
                .iter()
                .find(|(g, _)| *g == group)
                .map(|(_, gid)| *gid)
        }
    }

    fn guest() -> FakePasswd {
        FakePasswd {
            users: vec![("node", 1000, 20)],
            groups: vec![("staff", 50)],
        }
    }

    fn ids(spec: &str, passwd: &dyn Passwd) -> (u32, u32) {
        let creds = resolve(spec, passwd).expect("this identity resolves");
        let (uid, gid) = creds.uid_gid();
        (uid.as_raw(), gid.as_raw())
    }

    #[test]
    fn a_named_user_resolves_against_the_guests_own_passwd() {
        assert_eq!(
            ids("node", &guest()),
            (1000, 20),
            "a name means whatever uid this image gave it, so the answer has to come from the guest rather than from the host that staged the run"
        );
    }

    #[test]
    fn a_named_user_takes_its_primary_group_when_none_is_declared() {
        let passwd = FakePasswd {
            users: vec![("node", 1000, 20)],
            groups: Vec::new(),
        };
        assert_eq!(
            ids("node", &passwd),
            (1000, 20),
            "the primary group is what the guest's own passwd line says, not a group that happens to share the user's name — SandboxCredentials::resolve would have found nothing here"
        );
    }

    #[test]
    fn a_numeric_user_with_no_passwd_line_still_resolves() {
        assert_eq!(
            ids("1500", &FakePasswd::default()),
            (1500, 1500),
            "a number is the uid directly, so an image carrying a numeric USER and no matching passwd line must still be able to run a script"
        );
    }

    #[test]
    fn a_declared_group_outranks_the_users_primary_one() {
        assert_eq!(ids("node:staff", &guest()), (1000, 50));
    }

    #[test]
    fn a_numeric_group_resolves_without_a_group_file_entry() {
        assert_eq!(ids("node:77", &guest()), (1000, 77));
    }

    #[test]
    fn a_numeral_group_is_the_gid_itself_even_when_a_group_is_named_for_it() {
        let passwd = FakePasswd {
            users: vec![("node", 1000, 20)],
            groups: vec![("77", 500)],
        };
        assert_eq!(
            ids("node:77", &passwd),
            (1000, 77),
            "§3.1.3 uses a number as the id directly and lns-init resolves the workload's group the same way, so an image that names a group for a numeral must not be able to give a script a gid the workload would never get"
        );
    }

    #[test]
    fn a_user_the_guest_cannot_resolve_is_an_error_rather_than_a_fallback() {
        let err = resolve("nobody-here", &guest()).expect_err("an unknown name has no answer");
        assert!(
            err.contains("nobody-here"),
            "falling back would run the script as an identity nobody named, and root is the one it would most likely fall back to; got: {err}"
        );
    }

    #[test]
    fn a_group_the_guest_cannot_resolve_is_an_error() {
        let err = resolve("node:ghosts", &guest()).expect_err("an unknown group has no answer");
        assert!(err.contains("ghosts"), "got: {err}");
    }

    #[test]
    fn a_spec_the_user_grammar_rejects_is_reported_as_such() {
        for spec in ["", "node:staff:extra", ":staff", "node:"] {
            assert!(
                resolve(spec, &guest()).is_err(),
                "the document format already refuses this shape, so reaching the guest with it is a bug worth naming: {spec:?}"
            );
        }
    }
}
