Feature: users discover the CLI surface
  Help text is how operators learn what `lns` and its subcommands
  accept. The CLI must surface usage via the conventional clap
  idioms — `lns --help`, `lns help <sub>`, `lns <sub> --help` — and
  via a bare invocation (which is a missing-subcommand error that
  still prints the usage line).

  Scenario: bare invocation surfaces usage
    When I run "lns"
    Then the exit code is 2
    And the output contains "Usage: lns"

  Scenario: --help exits cleanly with usage
    When I run "lns --help"
    Then the exit code is 0
    And the output contains "Usage: lns"

  Scenario: lns help run prints the run usage
    When I run "lns help run"
    Then the exit code is 0
    And the output contains "Usage: lns run"

  Scenario: lns help audit prints the audit usage
    When I run "lns help audit"
    Then the exit code is 0
    And the output contains "Usage: lns audit"

  Scenario: lns run --help prints usage and key flags
    When I run "lns run --help"
    Then the exit code is 0
    And the output contains "Usage: lns run"
    And the output contains "REF"
    And the output contains "./lns.yaml"
    And the output contains "--cpus"
    And the output contains "--mem"
    And the output contains "--policy"

  Scenario: lns audit --help describes the unified timeline
    When I run "lns audit --help"
    Then the exit code is 0
    And the output contains "Usage: lns audit"
    And the output contains "SANDBOX"
    And the output contains "--connector"
    And the output contains "--kind"
    And the output contains "--json"
