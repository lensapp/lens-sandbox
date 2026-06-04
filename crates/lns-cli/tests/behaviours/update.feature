Feature: lns update CLI surface
  `lns update` lets operators refresh the installed `lns` and
  `lns-service` binaries from the CDN-published release tarball.
  The surface is mostly side-effect-laden (network + filesystem +
  service lifecycle), so Layer 2 pins clap behaviour here and the
  orchestration lives behind in-process unit tests with mocked
  ports.

  Scenario: lns update --help describes the subcommand
    When I run "lns update --help"
    Then the exit code is 0
    And the output contains "Usage: lns update"
    And the output contains "--force"

  Scenario: lns help update prints the update usage
    When I run "lns help update"
    Then the exit code is 0
    And the output contains "Usage: lns update"

  Scenario: bare --help lists the update subcommand
    When I run "lns --help"
    Then the exit code is 0
    And the output contains "update"

  Scenario: lns update --force is accepted by clap
    When I run "lns update --force"
    Then the exit code is 0

  Scenario: lns update rejects unknown flags
    When I run "lns update --not-a-real-flag"
    Then the exit code is 2
    And the output contains "unexpected argument"
