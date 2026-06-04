use super::*;
use crate::cli::{Command, UpdateArgs};

#[test]
fn update_command_skips_the_announce_so_it_does_not_nag_during_an_upgrade() {
    let update = Command::Update(UpdateArgs {
        force: false,
        dry_run: false,
    });
    assert!(!should_announce(&update));
}

#[test]
fn other_commands_announce() {
    assert!(should_announce(&Command::Ls));
}

#[test]
fn announce_is_suppressed_when_the_kill_switch_is_set() {
    let set = |k: &str| (k == lns_ipc::NO_UPDATE_CHECK_ENV).then(|| "1".to_string());
    assert!(!announce_enabled(set));
}

#[test]
fn announce_is_enabled_when_the_kill_switch_is_absent() {
    assert!(announce_enabled(|_| None));
}

#[derive(Default)]
struct FakeReader {
    status: Option<UpdateStatus>,
    install_id: Option<String>,
}
impl StatusReader for FakeReader {
    fn read_status(&self) -> Option<UpdateStatus> {
        self.status.clone()
    }
    fn read_install_id(&self) -> Option<String> {
        self.install_id.clone()
    }
}

fn status(latest: &str, min_secure: Option<&str>) -> UpdateStatus {
    UpdateStatus {
        latest: latest.to_string(),
        min_secure_version: min_secure.map(str::to_string),
        checked_at_unix: 0,
    }
}

#[test]
fn build_payload_carries_only_id_version_os_arch() {
    let p = build_payload("abc".to_string());
    assert_eq!(p.install_id, "abc");
    assert_eq!(p.version, CURRENT_VERSION);
    assert_eq!(p.os, std::env::consts::OS);
    assert_eq!(p.arch, std::env::consts::ARCH);
    let json = serde_json::to_value(&p).unwrap();
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["arch", "install_id", "os", "version"]);
}

#[test]
fn classify_up_to_date_when_not_behind_latest() {
    assert_eq!(classify("1.0.0", "1.0.0", None), CheckOutcome::UpToDate);
    assert_eq!(classify("2.0.0", "1.0.0", None), CheckOutcome::UpToDate);
}

#[test]
fn classify_update_available_when_behind_latest() {
    assert_eq!(
        classify("1.0.0", "1.2.0", None),
        CheckOutcome::UpdateAvailable {
            latest: "1.2.0".to_string()
        }
    );
}

#[test]
fn classify_security_advisory_when_below_min_secure() {
    assert_eq!(
        classify("1.0.0", "1.2.0", Some("1.1.0")),
        CheckOutcome::SecurityAdvisory {
            latest: "1.2.0".to_string()
        }
    );
}

#[test]
fn classify_ignores_min_secure_once_at_or_above_it() {
    assert_eq!(
        classify("1.1.0", "1.2.0", Some("1.1.0")),
        CheckOutcome::UpdateAvailable {
            latest: "1.2.0".to_string()
        }
    );
}

#[test]
fn version_parsing_is_numeric_per_component_and_tolerates_garbage() {
    assert!(version_lt("1.9.0", "1.10.0"), "numeric, not lexical");
    assert!(!version_lt("1.0.0", "1.0.0"));
    assert!(version_lt("0.4.0", "0.5.0"));
    assert_eq!(parse_version("1.2.3"), (1, 2, 3));
    assert_eq!(parse_version("1.x"), (1, 0, 0));
    assert_eq!(parse_version(""), (0, 0, 0));
}

#[test]
fn render_up_to_date_is_silent() {
    let mut out = Vec::new();
    render_outcome(&CheckOutcome::UpToDate, &mut out).unwrap();
    assert!(out.is_empty());
}

#[test]
fn render_update_available_points_at_lns_update() {
    let mut out = Vec::new();
    render_outcome(
        &CheckOutcome::UpdateAvailable {
            latest: "9.9.9".to_string(),
        },
        &mut out,
    )
    .unwrap();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("9.9.9") && s.contains("lns update"));
}

#[test]
fn render_security_advisory_points_at_lns_update() {
    let mut out = Vec::new();
    render_outcome(
        &CheckOutcome::SecurityAdvisory {
            latest: "9.9.9".to_string(),
        },
        &mut out,
    )
    .unwrap();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("security") && s.contains("lns update"));
}

#[test]
fn announce_reports_marked_update() {
    let reader = FakeReader {
        status: Some(status("9.9.9", None)),
        ..Default::default()
    };
    let mut out = Vec::new();
    announce(&reader, "1.0.0", &mut out).unwrap();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("9.9.9") && s.contains("lns update"));
}

#[test]
fn announce_reports_security_update() {
    let reader = FakeReader {
        status: Some(status("9.9.9", Some("2.0.0"))),
        ..Default::default()
    };
    let mut out = Vec::new();
    announce(&reader, "1.0.0", &mut out).unwrap();
    let s = String::from_utf8(out).unwrap();
    assert!(s.to_lowercase().contains("security") && s.contains("lns update"));
}

#[test]
fn announce_is_silent_when_up_to_date() {
    let reader = FakeReader {
        status: Some(status(CURRENT_VERSION, None)),
        ..Default::default()
    };
    let mut out = Vec::new();
    announce(&reader, CURRENT_VERSION, &mut out).unwrap();
    assert!(out.is_empty());
}

#[test]
fn announce_is_silent_when_no_status_marked_yet() {
    let reader = FakeReader::default();
    let mut out = Vec::new();
    announce(&reader, "1.0.0", &mut out).unwrap();
    assert!(out.is_empty());
}

#[test]
fn dry_run_prints_persisted_id_and_sends_nothing() {
    let reader = FakeReader {
        install_id: Some("persisted-id".to_string()),
        ..Default::default()
    };
    let mut out = Vec::new();
    dry_run(&reader, &mut out).unwrap();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("persisted-id"));
    assert!(s.contains(CURRENT_VERSION));
}

#[test]
fn dry_run_shows_placeholder_when_no_id_yet() {
    let reader = FakeReader::default();
    let mut out = Vec::new();
    dry_run(&reader, &mut out).unwrap();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("pending") && s.contains("install_id"));
}
