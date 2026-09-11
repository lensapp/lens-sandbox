#[cfg(target_os = "linux")]
pub mod real;

#[derive(Debug, PartialEq, Eq)]
pub enum Launch {
    Primary,
    Exec { uid: u32, gid: u32 },
    Direct,
}

pub fn launcher_identity() -> (u32, u32, u32) {
    (1, 0, 0)
}

pub fn configure_loopback(
    mut configure: impl FnMut(&str, Option<[u8; 4]>) -> std::io::Result<()>,
) -> std::io::Result<()> {
    configure("lo", None)?;
    configure("lo:lns", Some([10, 200, 0, 1]))
}

pub fn seal(fd: i32, apply: impl FnOnce(i32, i32) -> std::io::Result<()>) -> std::io::Result<()> {
    const IMMUTABLE_SEALS: i32 = 15;
    apply(fd, IMMUTABLE_SEALS)
}

pub fn route<T>(
    isolated: bool,
    enter: impl FnOnce() -> std::io::Result<()>,
    connect: impl FnOnce() -> std::io::Result<T>,
) -> std::io::Result<T> {
    if isolated {
        enter()?;
    }
    connect()
}

pub fn select(primary: bool, isolated: bool, confined: bool, uid: u32, gid: u32) -> Launch {
    if primary {
        Launch::Primary
    } else if isolated && confined {
        Launch::Exec { uid, gid }
    } else {
        Launch::Direct
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_address_uses_an_alias_so_localhost_remains_available() {
        let mut calls = Vec::new();
        configure_loopback(|name, address| {
            calls.push((name.to_owned(), address));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            calls,
            [
                ("lo".into(), None),
                ("lo:lns".into(), Some([10, 200, 0, 1]))
            ]
        );
    }

    #[test]
    fn interface_setup_stops_on_the_first_failure() {
        for failed_step in [1, 2] {
            let mut calls = 0;
            let error = configure_loopback(|_, _| {
                calls += 1;
                if calls == failed_step {
                    Err(std::io::Error::other("interface setup failed"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert_eq!(error.to_string(), "interface setup failed");
            assert_eq!(calls, failed_step);
        }
    }

    #[test]
    fn no_workload_uid_matches_all_privileged_launcher_ids_across_exec() {
        let (real, effective, saved) = launcher_identity();
        assert_eq!(effective, 0);
        assert_eq!(saved, 0);
        for uid in [0, 1, 1000, 65534] {
            assert!(
                !(real == uid && effective == uid && saved == uid),
                "workload uid {uid} can match the privileged launcher credentials"
            );
        }
    }

    #[test]
    fn forwarding_enters_the_workload_namespace_before_connecting_and_fails_closed() {
        let order = std::cell::RefCell::new(Vec::new());
        let entry_failure = std::cell::Cell::new(false);
        let connect_failure = std::cell::Cell::new(false);
        let enter = || {
            order.borrow_mut().push("enter");
            if entry_failure.get() {
                Err(std::io::Error::other("setns refused"))
            } else {
                Ok(())
            }
        };
        let connect = || {
            order.borrow_mut().push("connect");
            if connect_failure.get() {
                Err(std::io::Error::other("connection refused"))
            } else {
                Ok(7)
            }
        };
        assert_eq!(route(true, enter, connect).unwrap(), 7);
        assert_eq!(order.replace(Vec::new()), ["enter", "connect"]);
        entry_failure.set(true);
        assert_eq!(
            route(true, enter, connect).unwrap_err().to_string(),
            "setns refused"
        );
        assert_eq!(order.replace(Vec::new()), ["enter"]);
        assert_eq!(route(false, enter, connect).unwrap(), 7);
        assert_eq!(order.replace(Vec::new()), ["connect"]);
        entry_failure.set(false);
        connect_failure.set(true);
        assert_eq!(
            route(true, enter, connect).unwrap_err().to_string(),
            "connection refused"
        );
        assert_eq!(order.replace(Vec::new()), ["enter", "connect"]);
    }

    #[test]
    fn launcher_snapshot_is_immutable_or_launch_fails() {
        let result = seal(17, |fd, flags| {
            assert_eq!(fd, 17);
            assert_eq!(
                flags, 15,
                "launcher must seal writes, growth, shrinkage, and further seal changes"
            );
            Err(std::io::Error::other("sealing failed"))
        });
        assert_eq!(result.unwrap_err().to_string(), "sealing failed");
        assert!(seal(18, |_, _| Ok(())).is_ok());
    }

    #[test]
    fn supervisor_and_confined_exec_use_the_trusted_launcher() {
        assert_eq!(select(true, false, false, 0, 0), Launch::Primary);
        for uid in [0, 1000] {
            assert_eq!(
                select(false, true, true, uid, 100),
                Launch::Exec { uid, gid: 100 }
            );
        }
        assert_eq!(select(false, false, true, 1000, 1000), Launch::Direct);
        assert_eq!(select(false, true, false, 0, 0), Launch::Direct);
    }
}
