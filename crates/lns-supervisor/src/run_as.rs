/// How the supervisor should hand its privileges to the workload.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Plan {
    RootWithCapsDropped,
    Setuid { uid: u32, gid: u32 },
}

pub(crate) fn plan(uid: Option<u32>, gid: Option<u32>) -> Plan {
    match (uid, gid) {
        (Some(uid), Some(gid)) if uid != ROOT_UID => Plan::Setuid { uid, gid },
        _ => Plan::RootWithCapsDropped,
    }
}

const ROOT_UID: u32 = 0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_run_as_keeps_root_but_drops_capabilities() {
        assert_eq!(
            plan(Some(0), Some(0)),
            Plan::RootWithCapsDropped,
            "setuid(0) is a no-op that leaves CAP_NET_ADMIN and CAP_SYS_PTRACE in place, so a root run-as must take the cap-drop path instead"
        );
    }

    #[test]
    fn a_root_uid_under_a_non_root_group_still_drops_capabilities() {
        assert_eq!(
            plan(Some(0), Some(20)),
            Plan::RootWithCapsDropped,
            "the kernel only zeroes the capability sets on setuid to a non-zero uid — the gid does not enter into it"
        );
    }

    #[test]
    fn a_non_root_run_as_setuids_to_the_resolved_ids() {
        assert_eq!(
            plan(Some(65534), Some(65534)),
            Plan::Setuid {
                uid: 65534,
                gid: 65534
            }
        );
    }

    #[test]
    fn a_non_root_uid_in_the_root_group_still_setuids() {
        assert_eq!(
            plan(Some(1000), Some(0)),
            Plan::Setuid { uid: 1000, gid: 0 },
            "setuid(1000) zeroes the capability sets even when the workload stays in group 0"
        );
    }

    #[test]
    fn ids_lns_init_could_not_resolve_keep_root_with_capabilities_dropped() {
        assert_eq!(plan(None, None), Plan::RootWithCapsDropped);
        assert_eq!(plan(Some(1000), None), Plan::RootWithCapsDropped);
        assert_eq!(plan(None, Some(1000)), Plan::RootWithCapsDropped);
    }
}
