//! A leaf over nix's NSS lookups; the ladder that reads it is host-tested against a fake in `ids.rs`.

use super::Passwd;

pub(crate) struct GuestPasswd;

impl Passwd for GuestPasswd {
    fn uid_of(&self, name: &str) -> Option<u32> {
        nix::unistd::User::from_name(name)
            .ok()
            .flatten()
            .map(|user| user.uid.as_raw())
    }

    fn primary_gid_of(&self, name: &str) -> Option<u32> {
        nix::unistd::User::from_name(name)
            .ok()
            .flatten()
            .map(|user| user.gid.as_raw())
    }

    fn gid_of_group(&self, group: &str) -> Option<u32> {
        nix::unistd::Group::from_name(group)
            .ok()
            .flatten()
            .map(|g| g.gid.as_raw())
    }
}
