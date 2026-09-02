@microvm
Feature: session streaming verbs reach a real guest without wedging the terminal
  run, exec, logs, and attach stream over tokio stdin/stdout, so their
  dispatch must run with the std stdin/stdout locks free. These scenarios
  drive the streaming verbs — both spellings where each has two — through
  the real binaries against a booted guest — a terminal-ownership regression
  deadlocks the CLI and trips the harness timeout.

  Scenario: a non-interactive exec through the top level returns the command's output
    Given the LNS service is running
    When the user starts a detached microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox sleep 60'"
    Then the exit code is 0
    When the user execs "/bin/echo exec-hi" in that run
    Then the exit code is 0
    And the output contains "exec-hi"

  Scenario: a non-interactive exec through the sandbox namespace returns the command's output
    Given the LNS service is running
    When the user starts a detached microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox sleep 60'"
    Then the exit code is 0
    When the user execs "/bin/echo exec-hi" in that run via the sandbox namespace
    Then the exit code is 0
    And the output contains "exec-hi"

  Scenario: non-interactive exec preserves streams and exit status without entering primary logs
    Given the LNS service is running
    When the user starts a detached microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox sleep 60'"
    Then the exit code is 0
    When the user execs "/bin/sh -c 'echo exec-out; echo exec-err >&2; exit 7'" in that run
    Then the exit code is 7
    And the output contains "exec-out"
    And the output contains "exec-err"
    When the user prints that run's logs
    Then the exit code is 0
    And the output does not contain "exec-out"
    And the output does not contain "exec-err"

  Scenario: an exec adopts the workload user's identity instead of the broker's
    Given the LNS service is running
    When the user starts a detached microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox sleep 60'"
    Then the exit code is 0
    When the user execs "/bin/sh -c 'echo home=$HOME user=$USER cwd=$(pwd)end'" in that run
    Then the exit code is 0
    And the output contains "home=/home/sandbox"
    And the output contains "user=sandbox"
    And the output contains "cwd=/end"

  Scenario: logs without follow replays captured output and returns through the top level
    Given the LNS service is running
    When the user starts a detached microVM command "/bin/sh -c 'echo hello-logs && /.lens/guest-tools/bin/busybox sleep 60'"
    Then the exit code is 0
    When the user prints that run's logs until they contain "hello-logs"
    Then the exit code is 0
    And the output contains "hello-logs"

  Scenario: logs without follow replays captured output and returns through the sandbox namespace
    Given the LNS service is running
    When the user starts a detached microVM command "/bin/sh -c 'echo hello-logs && /.lens/guest-tools/bin/busybox sleep 60'"
    Then the exit code is 0
    When the user prints that run's logs via the sandbox namespace until they contain "hello-logs"
    Then the exit code is 0
    And the output contains "hello-logs"

  Scenario: logs -f follows a detached run's output and returns when it exits
    Given the LNS service is running
    When the user starts a detached microVM command "/bin/sh -c 'echo follow-hi && /.lens/guest-tools/bin/busybox sleep 5 && echo follow-bye'"
    Then the exit code is 0
    When the user follows that run's logs until it exits
    Then the exit code is 0
    And the output contains "follow-hi"
    And the output contains "follow-bye"

  Scenario: attach re-joins a detached run's live output and adopts its exit code
    Given the LNS service is running
    When the user starts a detached microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox sleep 5 && echo attach-bye && exit 7'"
    Then the exit code is 0
    When the user attaches to that run until it exits
    Then the exit code is 7
    And the output contains "attach-bye"

  Scenario: sandbox run boots the definition exactly like its top-level shortcut
    Given the LNS service is running
    When the user runs a microVM command "/bin/echo sandbox-run-hi" via the sandbox namespace
    Then the exit code is 0
    And the output contains "sandbox-run-hi"
