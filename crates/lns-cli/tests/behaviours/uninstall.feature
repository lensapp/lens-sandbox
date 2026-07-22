Feature: lns uninstall CLI surface
  `lns uninstall` stops running sandboxes, stops the background service,
  removes login auto-start, and deletes the installed `lns` and
  `lns-service` binaries; `--purge` additionally deletes all local data.
  The command is destructive and side-effect-laden (service lifecycle +
  filesystem), so Layer 2 pins clap behaviour here and the orchestration
  lives behind in-process unit tests with mocked ports.

  Scenario: lns uninstall --help describes the subcommand and the purge opt-in
    When I run "lns uninstall --help"
    Then the exit code is 0
    And the output contains "Usage: lns uninstall"
    And the output contains "--purge"

  Scenario: lns uninstall --help documents the confirmation opt-out
    When I run "lns uninstall --help"
    Then the exit code is 0
    And the output contains "--yes"

  Scenario: lns help uninstall prints the uninstall usage
    When I run "lns help uninstall"
    Then the exit code is 0
    And the output contains "Usage: lns uninstall"

  Scenario: bare --help lists the uninstall subcommand
    When I run "lns --help"
    Then the exit code is 0
    And the output contains "uninstall"

  Scenario: lns uninstall --purge --yes is accepted by clap
    When I run "lns uninstall --purge --yes"
    Then the exit code is 0

  Scenario: lns uninstall rejects unknown flags
    When I run "lns uninstall --not-a-real-flag"
    Then the exit code is 2
    And the output contains "unexpected argument"
