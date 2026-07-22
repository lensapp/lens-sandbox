use lns_ipc::StatusInfo;

pub(crate) const CLI_PROTOCOL: u32 = lns_ipc::IPC_PROTOCOL_VERSION;
pub(crate) const CLI_BUILD: &str = env!("LNS_BUILD_SHA");

pub(crate) const REMEDY: &str =
    "reinstall Lens Sandbox or run `lns service restart` to reconcile the two binaries";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Compat {
    Match,
    BuildDrift { running: String },
    ProtocolMismatch { cli: u32, running: Option<u32> },
}

/// Precondition: the caller already confirmed the service is alive (Ping), so a
/// `None` status means "alive but its Status is undecodable" — an old,
/// wire-incompatible service — not "not running".
pub(crate) fn classify(cli_protocol: u32, cli_build: &str, running: Option<&StatusInfo>) -> Compat {
    match running {
        None => Compat::ProtocolMismatch {
            cli: cli_protocol,
            running: None,
        },
        Some(s) if s.protocol != cli_protocol => Compat::ProtocolMismatch {
            cli: cli_protocol,
            running: Some(s.protocol),
        },
        Some(s) if s.build != cli_build => Compat::BuildDrift {
            running: s.build.clone(),
        },
        Some(_) => Compat::Match,
    }
}

pub(crate) fn build_drift_message(running: &str, cli: &str) -> String {
    format!(
        "the running lns-service is a different build ({running}) than this lns ({cli}); {REMEDY}."
    )
}

fn protocol_mismatch_message(cli: u32, running: Option<u32>) -> String {
    let running = running.map_or_else(
        || "an older, unreadable version".to_string(),
        |p| p.to_string(),
    );
    format!(
        "the running lns-service speaks IPC protocol {running} but this lns speaks {cli}; \
         they are incompatible. {REMEDY}."
    )
}

/// For `lns service start` against an already-running service: `None` means a
/// clean already-running (print normally); `Some(warning)` means a build or
/// protocol mismatch the user should reconcile with `lns service restart`.
pub(crate) fn already_running_warning(compat: &Compat) -> Option<String> {
    match compat {
        Compat::Match => None,
        Compat::BuildDrift { running } => Some(format!(
            "Lens Sandbox is already running as a different build ({running}) than this lns ({CLI_BUILD}); \
             run `lns service restart` to adopt the freshly installed binary."
        )),
        Compat::ProtocolMismatch { running, .. } => {
            let running = running.map_or_else(
                || "an older version".to_string(),
                |p| format!("protocol {p}"),
            );
            Some(format!(
                "Lens Sandbox is already running but speaks an incompatible IPC version ({running}); \
                 run `lns service restart` to adopt the freshly installed binary."
            ))
        }
    }
}

/// For `lns service restart` after the login agent relaunched: `None` means the
/// relaunch reconciled the pair; `Some(warning)` means the agent relaunched a
/// stale binary and the mismatch is still in place.
pub(crate) fn relaunched_warning(
    compat: &Compat,
    agent_path: Option<&std::path::Path>,
) -> Option<String> {
    let still = match compat {
        Compat::Match => return None,
        Compat::BuildDrift { running } => {
            format!("still a different build ({running}) than this lns ({CLI_BUILD})")
        }
        Compat::ProtocolMismatch { cli, running } => {
            let running = running.map_or_else(
                || "an older, unreadable version".to_string(),
                |p| format!("protocol {p}"),
            );
            format!("still speaking an incompatible IPC version ({running}; this lns speaks {cli})")
        }
    };
    let agent = agent_path.map_or_else(
        || "the login agent".to_string(),
        |p| format!("the login agent at {}", p.display()),
    );
    Some(format!(
        "Lens Sandbox restarted, but the running service is {still}; \
         {agent} points at a stale binary — run `lns service enable` to repoint it."
    ))
}

/// Gate for the substantive-command path: `Err` aborts the command; a build-only
/// drift logs one warning and proceeds; a matching pair is silent.
pub(crate) fn enforce_for_command(compat: Compat) -> Result<(), String> {
    match compat {
        Compat::Match => Ok(()),
        Compat::BuildDrift { running } => {
            crate::log::warn!("{}", build_drift_message(&running, CLI_BUILD));
            Ok(())
        }
        Compat::ProtocolMismatch { cli, running } => Err(protocol_mismatch_message(cli, running)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(protocol: u32, build: &str) -> StatusInfo {
        StatusInfo {
            pid: 1,
            uptime_secs: 1,
            version: "0.0.0".into(),
            protocol,
            build: build.into(),
        }
    }

    #[test]
    fn matching_protocol_and_build_is_a_match() {
        let s = status(7, "deadbeef");
        assert_eq!(classify(7, "deadbeef", Some(&s)), Compat::Match);
    }

    #[test]
    fn matching_protocol_but_differing_build_is_drift() {
        let s = status(7, "olddddd");
        assert_eq!(
            classify(7, "newwwww", Some(&s)),
            Compat::BuildDrift {
                running: "olddddd".into()
            }
        );
    }

    #[test]
    fn differing_protocol_is_a_mismatch_naming_both_versions() {
        let s = status(2, "whatever");
        assert_eq!(
            classify(7, "whatever", Some(&s)),
            Compat::ProtocolMismatch {
                cli: 7,
                running: Some(2)
            }
        );
    }

    #[test]
    fn an_alive_service_with_no_readable_status_is_a_mismatch() {
        assert_eq!(
            classify(7, "deadbeef", None),
            Compat::ProtocolMismatch {
                cli: 7,
                running: None
            }
        );
    }

    #[test]
    fn enforce_is_silent_and_ok_on_a_match() {
        let events = crate::test_env::capture_events(|| {
            enforce_for_command(Compat::Match).expect("a match proceeds");
        });
        assert!(
            events.is_empty(),
            "a matching pair must not warn: {events:?}"
        );
    }

    #[test]
    fn enforce_warns_exactly_once_and_proceeds_on_build_drift() {
        let events = crate::test_env::capture_events(|| {
            enforce_for_command(Compat::BuildDrift {
                running: "0ldbuild".into(),
            })
            .expect("build drift is not fatal");
        });
        assert_eq!(events.len(), 1, "exactly one warning: {events:?}");
        assert!(events[0].contains("0ldbuild"), "{:?}", events[0]);
        assert!(events[0].contains("lns service restart"), "{:?}", events[0]);
    }

    #[test]
    fn enforce_hard_fails_with_the_remedy_on_protocol_mismatch() {
        let err = enforce_for_command(Compat::ProtocolMismatch {
            cli: 7,
            running: Some(2),
        })
        .expect_err("a protocol mismatch must abort the command");
        assert!(err.contains('7') && err.contains('2'), "names both: {err}");
        assert!(err.contains(REMEDY), "carries the remedy: {err}");
    }

    #[test]
    fn protocol_mismatch_message_handles_an_unreadable_running_version() {
        let msg = protocol_mismatch_message(7, None);
        assert!(msg.contains("unreadable"), "{msg}");
        assert!(msg.contains(REMEDY), "{msg}");
    }

    #[test]
    fn relaunched_warning_is_none_when_the_relaunch_reconciled_the_pair() {
        assert!(relaunched_warning(&Compat::Match, None).is_none());
    }

    #[test]
    fn relaunched_warning_names_the_stale_build_the_agent_path_and_the_repoint_remedy() {
        let w = relaunched_warning(
            &Compat::BuildDrift {
                running: "0ldbuild".into(),
            },
            Some(std::path::Path::new("/Users/alice/Library/LaunchAgents/run.lns.service.plist")),
        )
        .expect("a persisting drift after relaunch must warn");
        assert!(w.contains("0ldbuild"), "{w}");
        assert!(w.contains("run.lns.service.plist"), "{w}");
        assert!(w.contains("lns service enable"), "{w}");
        assert!(!w.contains("restarted."), "must not read as success: {w}");
    }

    #[test]
    fn relaunched_warning_covers_protocol_mismatch_and_an_unresolvable_agent_path() {
        let known = relaunched_warning(
            &Compat::ProtocolMismatch {
                cli: 7,
                running: Some(2),
            },
            None,
        )
        .expect("a persisting mismatch after relaunch must warn");
        assert!(known.contains("protocol 2"), "{known}");
        assert!(known.contains("the login agent points"), "{known}");

        let unknown = relaunched_warning(
            &Compat::ProtocolMismatch {
                cli: 7,
                running: None,
            },
            None,
        )
        .expect("mismatch must warn");
        assert!(unknown.contains("unreadable"), "{unknown}");
    }

    #[test]
    fn already_running_warning_is_none_on_a_match() {
        assert!(already_running_warning(&Compat::Match).is_none());
    }

    #[test]
    fn already_running_warning_names_the_drifted_build_and_suggests_restart() {
        let w = already_running_warning(&Compat::BuildDrift {
            running: "0ldbuild".into(),
        })
        .expect("drift must warn");
        assert!(w.contains("0ldbuild"), "{w}");
        assert!(w.contains("lns service restart"), "{w}");
    }

    #[test]
    fn already_running_warning_covers_protocol_mismatch_known_and_unknown() {
        let known = already_running_warning(&Compat::ProtocolMismatch {
            cli: 7,
            running: Some(2),
        })
        .expect("mismatch must warn");
        assert!(known.contains("protocol 2"), "{known}");
        assert!(known.contains("lns service restart"), "{known}");

        let unknown = already_running_warning(&Compat::ProtocolMismatch {
            cli: 7,
            running: None,
        })
        .expect("mismatch must warn");
        assert!(unknown.contains("older version"), "{unknown}");
    }
}
