use super::*;
use crate::fixtures::source::FixtureSource;
use crate::fixtures::{ConnRecord, Fixtures, Sizes};
use crate::result::Status;
use anyhow::bail;

fn output(stdout: &str, code: i32) -> Output {
    Output {
        stdout: stdout.to_string(),
        stderr: String::new(),
        code,
        duration: Duration::from_millis(1),
        timed_out: false,
    }
}

#[test]
fn every_case_of_this_phase_is_registered_once_and_the_shutdown_case_runs_last() {
    let names = names();
    assert_eq!(names.len(), 13);
    assert_eq!(names[0], PREFLIGHT);
    assert_eq!(names[1], "lease-and-resolver");
    assert_eq!(names[names.len() - 1], "service-stop-with-guests");

    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "a case name is registered twice");
}

#[test]
fn a_case_records_its_outcome_and_how_long_it_took() {
    let passed = timed("upload-100m", |case| {
        case.record("bytes", 10u64);
        Ok(Outcome::Pass)
    });
    assert_eq!(passed.status, Status::Pass);
    assert_eq!(passed.measures["bytes"].to_string(), "10");

    let failed = timed("upload-100m", |_| Ok(Outcome::Fail("short read".into())));
    assert_eq!(failed.status, Status::Fail);
    assert_eq!(failed.error.as_deref(), Some("short read"));

    let skipped = timed("udp-echo", |_| Ok(Outcome::Skip("no sender".into())));
    assert_eq!(skipped.status, Status::Skip);

    let blocked = timed("create-destroy-20", |_| Ok(Outcome::Blocked("#427".into())));
    assert_eq!(blocked.status, Status::BlockedByProduct);
}

#[test]
fn a_case_that_could_not_drive_the_cli_fails_with_the_error_it_hit() {
    let case = timed("upload-100m", |_| bail!("no lns binary at /nowhere/lns"));
    assert_eq!(case.status, Status::Fail);
    assert!(
        case.error.as_deref().unwrap().contains("/nowhere/lns"),
        "{case:?}"
    );
}

#[test]
fn a_case_verifies_the_commands_it_relies_on_inside_the_guest() {
    let prelude = requires(&["dd", "nc"]);
    assert!(prelude.contains("command -v $c"), "{prelude}");
    assert!(prelude.contains("for c in dd nc"), "{prelude}");

    let missing = output(&format!("{MISSING_MARKER}nc\n"), 42);
    assert_eq!(
        missing_command(&missing).as_deref(),
        Some("the guest image has no `nc`")
    );
    assert_eq!(missing_command(&output("104857600\n", 0)), None);
}

#[test]
fn a_marked_field_is_read_back_out_of_the_guests_output() {
    assert_eq!(
        field("PARITY_EXIT=1 PARITY_ELAPSED=2\n", "PARITY_EXIT="),
        Some("1".to_string())
    );
    assert_eq!(field("nothing here\n", "PARITY_EXIT="), None);
}

#[test]
fn the_byte_count_comes_from_the_line_dd_wrote_to_stderr() {
    let stderr = "204800+0 records in\n204800+0 records out\n104857600 bytes (100.0MB) copied, 5.1 seconds, 19.6MB/s\n";
    assert_eq!(parse_dd_bytes(stderr), Some(104_857_600));
    assert_eq!(parse_dd_bytes("204800+0 records in\n"), None);
}

#[test]
fn throughput_is_measured_over_the_lifetime_of_the_fixtures_connection() {
    let record = ConnRecord {
        opened_ms: 1_000,
        closed_ms: Some(3_000),
        ..ConnRecord::default()
    };
    let rate = throughput_mib_s(100 * crate::fixtures::pattern::MIB, &record).unwrap();
    assert!((rate - 50.0).abs() < 0.001, "{rate}");

    let open = ConnRecord {
        opened_ms: 1_000,
        closed_ms: None,
        ..ConnRecord::default()
    };
    assert_eq!(throughput_mib_s(1, &open), None);

    let instant = ConnRecord {
        opened_ms: 1_000,
        closed_ms: Some(1_000),
        ..ConnRecord::default()
    };
    assert_eq!(throughput_mib_s(1, &instant), None);
}

#[test]
fn what_the_fixture_saw_is_recorded_on_both_ends() {
    let record = ConnRecord {
        bytes_in: 10,
        bytes_out: 20,
        sha_in: Some("aa".into()),
        sha_out: Some("bb".into()),
        saw_eof: true,
        error: Some("broken pipe".into()),
        ..ConnRecord::default()
    };
    let mut case = CaseResult::new("upload-100m");
    record_transfer(&mut case, "sink", &record);

    assert_eq!(case.measures["sink_bytes_in"].to_string(), "10");
    assert_eq!(case.measures["sink_bytes_out"].to_string(), "20");
    assert_eq!(case.measures["sink_sha_in"].to_string(), "aa");
    assert_eq!(case.measures["sink_sha_out"].to_string(), "bb");
    assert_eq!(case.measures["sink_saw_eof"].to_string(), "true");
    assert_eq!(case.measures["sink_error"].to_string(), "broken pipe");
}

#[test]
fn the_guests_exit_code_and_the_tail_of_its_output_are_kept() {
    let mut case = CaseResult::new("reset-mid-transfer");
    let long = "x".repeat(500);
    guest_output(&mut case, &output(&long, 1));

    assert_eq!(case.measures["guest_exit_code"].to_string(), "1");
    let tail = case.measures["guest_output_tail"].to_string();
    assert_eq!(tail.len(), 400);
    assert!(tail.chars().all(|c| c == 'x'), "{tail}");
}

#[test]
fn polling_returns_as_soon_as_the_fixture_recorded_what_the_case_waits_for() {
    let mut calls = 0;
    assert_eq!(
        poll(Duration::from_secs(5), || {
            calls += 1;
            (calls >= 2).then_some(calls)
        }),
        Some(2)
    );
    assert_eq!(
        poll(Duration::from_millis(200), || Option::<u8>::None),
        None
    );
}

#[test]
fn the_context_writes_one_definition_per_image_it_runs() {
    let dir = tempfile::tempdir().unwrap();
    let fixtures = FixtureSource::InProcess(
        Fixtures::start(std::net::Ipv4Addr::LOCALHOST, 0, Sizes::default()).unwrap(),
    );
    let lns = Lns::new(PathBuf::from("/bin/echo"), BTreeMap::new());
    let images = Images::default();
    let ctx = Ctx::prepare(
        &lns,
        &fixtures,
        &images,
        &dir.path().join("project"),
        "192.168.127".to_string(),
        BTreeMap::from([("loopback-witness".to_string(), "refused".to_string())]),
        Some(4242),
    )
    .unwrap();

    let alpine = std::fs::read_to_string(ctx.project.join("alpine.yaml")).unwrap();
    let curl = std::fs::read_to_string(ctx.project.join("curl.yaml")).unwrap();
    assert!(alpine.contains(&images.alpine), "{alpine}");
    assert!(curl.contains(&images.curl), "{curl}");
    for document in [&alpine, &curl] {
        for destination in fixtures.guest_destinations() {
            assert!(
                document.contains(&format!("match: \"{destination}\"")),
                "{document}"
            );
        }
    }
    assert_eq!(ctx.expectation("loopback-witness"), Some("refused"));
    assert_eq!(ctx.expectation("upload-100m"), None);
    assert_eq!(ctx.lan(), std::net::Ipv4Addr::LOCALHOST);
    assert_eq!(ctx.conn_mark(), 0);
    assert!(ctx.new_connection(Role::Sink, 0).is_none());
}

#[test]
fn every_case_declares_a_budget_and_whether_it_needs_a_raw_stream_to_the_fixture() {
    for case in all() {
        assert!(
            case.budget >= Duration::from_secs(60),
            "{} declares {:?}",
            case.name,
            case.budget
        );
    }
    assert!(needs_fixture_stream("upload-100m"));
    assert!(needs_fixture_stream("create-destroy-20"));
    assert!(!needs_fixture_stream(PREFLIGHT));
    assert!(!needs_fixture_stream("lease-and-resolver"));
    assert!(!needs_fixture_stream("loopback-witness"));
}

#[test]
fn the_transfer_budget_is_the_one_the_hundred_megabyte_size_is_measured_against() {
    let budget = |name: &str| {
        all()
            .into_iter()
            .find(|case| case.name == name)
            .map(|case| case.budget)
            .unwrap()
    };
    assert_eq!(budget("upload-100m"), Duration::from_secs(120));
    assert_eq!(budget("download-100m"), Duration::from_secs(120));
    assert_eq!(
        budget("create-destroy-20"),
        Duration::from_secs(60 * (lifecycle::CYCLES / lifecycle::CYCLE_GROUP) as u64)
    );
}

#[test]
fn what_the_fixtures_saw_is_recorded_on_the_case_whatever_its_verdict() {
    let mut case = CaseResult::new("upload-100m");
    record_activity(
        &mut case,
        crate::fixtures::Activity {
            connections: 2,
            bytes_in: 1024,
            bytes_out: 0,
        },
    );

    assert_eq!(case.measures["fixture_seen_connections"].to_string(), "2");
    assert_eq!(case.measures["fixture_seen_bytes_in"].to_string(), "1024");
    assert_eq!(case.measures["fixture_seen_bytes_out"].to_string(), "0");
}

#[test]
fn a_guest_command_killed_at_its_budget_says_so_in_the_case() {
    let mut case = CaseResult::new("upload-100m");
    let mut killed = output("started\n", -1);
    killed.timed_out = true;
    guest_output(&mut case, &killed);

    assert_eq!(case.measures["guest_timed_out"].to_string(), "true");
}

#[test]
fn a_case_run_outside_the_runner_still_has_a_budget_to_spend() {
    let dir = tempfile::tempdir().unwrap();
    let fixtures = FixtureSource::InProcess(
        Fixtures::start(std::net::Ipv4Addr::LOCALHOST, 0, Sizes::default()).unwrap(),
    );
    let lns = Lns::new(PathBuf::from("/bin/echo"), BTreeMap::new());
    let images = Images::default();
    let ctx = Ctx::prepare(
        &lns,
        &fixtures,
        &images,
        &dir.path().join("project"),
        "192.168.127".to_string(),
        BTreeMap::new(),
        None,
    )
    .unwrap();
    assert!(ctx.remaining() > Duration::from_secs(60));

    ctx.begin_case(Duration::from_secs(30));
    assert!(ctx.remaining() <= Duration::from_secs(30));
    assert!(ctx.within_budget(Duration::from_secs(180)) <= Duration::from_secs(30));
    assert_eq!(
        ctx.within_budget(Duration::from_millis(1)),
        Duration::from_millis(1)
    );
    assert_eq!(ctx.activity(), crate::fixtures::Activity::default());
}
