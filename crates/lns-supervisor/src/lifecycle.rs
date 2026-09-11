pub fn name(set: impl FnOnce(&str) -> std::io::Result<()>) -> std::io::Result<()> {
    set("lns-supervisor")
}

pub fn exit_code(code: Option<i32>, signal: Option<i32>) -> i32 {
    code.or_else(|| signal.map(|signal| 128 + signal))
        .unwrap_or(125)
}

pub fn harden(before_workload: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    before_workload()
}

pub fn forward_signal(
    process_group: u32,
    signal: lns_session::SignalKind,
    send: impl FnOnce(i32, i32) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let group = i32::try_from(process_group)
        .ok()
        .filter(|group| *group > 1)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid workload process group",
            )
        })?;
    match send(-group, signal.as_libc()) {
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(()),
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_launcher_keeps_the_supervisor_process_name_and_surfaces_failure() {
        let result = name(|value| {
            assert_eq!(value, "lns-supervisor");
            Err(std::io::Error::other("name refused"))
        });
        assert_eq!(result.unwrap_err().to_string(), "name refused");
        assert!(name(|_| Ok(())).is_ok());
    }

    #[test]
    fn forwards_to_the_whole_workload_group_not_only_the_shell() {
        let mut sent = None;
        forward_signal(123, lns_session::SignalKind::Term, |pid, sig| {
            sent = Some((pid, sig));
            Ok(())
        })
        .unwrap();
        assert_eq!(sent, Some((-123, 15)));
    }

    #[test]
    fn invalid_group_never_targets_the_supervisor_or_all_processes() {
        for pid in [0, 1, u32::MAX] {
            assert!(
                forward_signal(pid, lns_session::SignalKind::Int, |_, _| panic!(
                    "must not send"
                ))
                .is_err()
            );
        }
    }

    #[test]
    fn forwarding_tolerates_an_already_exited_group_but_not_other_errors() {
        assert!(
            forward_signal(123, lns_session::SignalKind::Term, |_, _| Err(
                std::io::Error::from_raw_os_error(libc::ESRCH)
            ))
            .is_ok()
        );
        let error = forward_signal(123, lns_session::SignalKind::Term, |_, _| {
            Err(std::io::Error::from_raw_os_error(libc::EPERM))
        })
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    }

    #[test]
    fn terminated_workload_preserves_the_signal_in_its_exit_code() {
        assert_eq!(exit_code(None, Some(2)), 130);
        assert_eq!(exit_code(None, Some(15)), 143);
        assert_eq!(exit_code(None, Some(9)), 137);
    }

    #[test]
    fn ordinary_and_missing_exit_statuses_are_distinct() {
        assert_eq!(exit_code(Some(0), None), 0);
        assert_eq!(exit_code(Some(17), None), 17);
        assert_eq!(exit_code(None, None), 125);
    }

    #[test]
    fn supervisor_hardening_failure_prevents_workload_startup() {
        let result = harden(|| Err("seccomp installation failed".into()));
        assert_eq!(result, Err("seccomp installation failed".into()));
    }

    #[test]
    fn successful_hardening_is_applied_not_skipped() {
        let mut applied = false;
        harden(|| {
            applied = true;
            Ok(())
        })
        .unwrap();
        assert!(applied);
    }
}
