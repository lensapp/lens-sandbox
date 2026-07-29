use lens_sandbox_core::privilege::SandboxCredentials;

const ROOT_UID: u32 = 0;

/// The identity to `setuid` into before exec; `None` keeps the supervisor's uid so the caller drops capabilities instead.
pub(crate) fn setuid_creds(creds: Option<&SandboxCredentials>) -> Option<SandboxCredentials> {
    creds
        .filter(|c| c.uid_gid().0.as_raw() != ROOT_UID)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds_for(uid: u32, gid: u32) -> SandboxCredentials {
        SandboxCredentials::resolve_by_uid(uid, gid).expect("resolve_by_uid never fails on a host")
    }

    #[test]
    fn a_root_run_as_is_not_a_setuid_target() {
        assert!(
            setuid_creds(Some(&creds_for(ROOT_UID, 0))).is_none(),
            "setuid(0) is a no-op that leaves CAP_NET_ADMIN and CAP_SYS_PTRACE in place, so a root run-as must reach the cap-drop path instead"
        );
    }

    #[test]
    fn a_root_uid_under_a_non_root_group_is_not_a_setuid_target() {
        assert!(
            setuid_creds(Some(&creds_for(ROOT_UID, 20))).is_none(),
            "the kernel only zeroes the capability sets on setuid to a non-zero uid — the gid does not enter into it"
        );
    }

    #[test]
    fn a_non_root_run_as_setuids_to_the_resolved_ids() {
        let creds =
            setuid_creds(Some(&creds_for(65534, 65534))).expect("a non-root run-as setuids");
        let (uid, gid) = creds.uid_gid();
        assert_eq!((uid.as_raw(), gid.as_raw()), (65534, 65534));
    }

    #[test]
    fn a_non_root_uid_in_the_root_group_still_setuids() {
        let creds = setuid_creds(Some(&creds_for(1000, 0)))
            .expect("setuid(1000) zeroes the capability sets even in group 0");
        let (uid, gid) = creds.uid_gid();
        assert_eq!((uid.as_raw(), gid.as_raw()), (1000, 0));
    }

    #[test]
    fn ids_lns_init_could_not_resolve_have_no_setuid_target() {
        assert!(setuid_creds(None).is_none());
    }
}
