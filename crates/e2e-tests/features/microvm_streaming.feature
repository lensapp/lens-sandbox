@microvm
Feature: session streaming verbs reach a real guest without wedging the terminal
  exec and logs stream over tokio stdin/stdout, so their dispatch must run
  with the std stdin/stdout locks free. These scenarios drive both spellings
  of each verb (`lns exec` / `lns sandbox exec`, `lns logs` / `lns sandbox logs`)
  through the real binaries against a booted guest — a terminal-ownership
  regression deadlocks the CLI and trips the harness timeout.

  Scenario: a non-interactive exec through the top level returns the command's output
    Given the Lens Sandbox service is running
    When the user starts a detached microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox sleep 60'"
    Then the exit code is 0
    When the user execs "/bin/echo exec-hi" in that run
    Then the exit code is 0
    And the output contains "exec-hi"

  Scenario: a non-interactive exec through the sandbox namespace returns the command's output
    Given the Lens Sandbox service is running
    When the user starts a detached microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox sleep 60'"
    Then the exit code is 0
    When the user execs "/bin/echo exec-hi" in that run via the sandbox namespace
    Then the exit code is 0
    And the output contains "exec-hi"

  Scenario: logs without follow replays captured output and returns through the top level
    Given the Lens Sandbox service is running
    When the user starts a detached microVM command "/bin/sh -c 'echo hello-logs && /.lens/guest-tools/bin/busybox sleep 60'"
    Then the exit code is 0
    When the user prints that run's logs until they contain "hello-logs"
    Then the exit code is 0
    And the output contains "hello-logs"

  Scenario: logs without follow replays captured output and returns through the sandbox namespace
    Given the Lens Sandbox service is running
    When the user starts a detached microVM command "/bin/sh -c 'echo hello-logs && /.lens/guest-tools/bin/busybox sleep 60'"
    Then the exit code is 0
    When the user prints that run's logs via the sandbox namespace until they contain "hello-logs"
    Then the exit code is 0
    And the output contains "hello-logs"
