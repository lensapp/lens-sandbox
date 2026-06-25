#[path = "specutil/mod.rs"]
mod specutil;

use specutil::{TIMEOUT_EXIT_CODE, capture_with_timeout};
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn capture_with_timeout_kills_a_child_that_outlives_the_deadline() {
    let mut cmd = Command::new("/bin/sleep");
    cmd.arg("30");
    let started = Instant::now();
    let result = capture_with_timeout(cmd, Duration::from_millis(300));
    let elapsed = started.elapsed();

    assert_eq!(
        result.exit_code, TIMEOUT_EXIT_CODE,
        "a killed run reports the timeout sentinel; stderr={:?}",
        result.stderr
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "the child is killed near the deadline, not awaited to completion (took {elapsed:?})"
    );
    assert!(
        result.stderr.contains("killed"),
        "the timeout is surfaced in stderr; stderr={:?}",
        result.stderr
    );
}

#[test]
fn capture_with_timeout_returns_output_and_code_when_the_child_finishes_first() {
    let mut cmd = Command::new("/bin/echo");
    cmd.arg("hello-from-host");
    let result = capture_with_timeout(cmd, Duration::from_secs(5));

    assert_eq!(result.exit_code, 0, "stderr={:?}", result.stderr);
    assert!(
        result.stdout.contains("hello-from-host"),
        "captured stdout should carry the child's output; stdout={:?}",
        result.stdout
    );
}
