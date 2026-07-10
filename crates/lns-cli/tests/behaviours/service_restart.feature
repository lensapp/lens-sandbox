Feature: lns service restart CLI surface
  `lns service restart` reconciles a running service with the freshly
  installed binary by stopping it and starting it again. It must be
  discoverable from the service help and parse cleanly. Behavioural
  depth (the stop→start orchestration, the version/protocol handshake,
  and the build-drift warning) is pinned by Layer 3 unit tests, because
  the Layer 2 harness only parses argv — it never dispatches to the
  service.

  Scenario: service help lists restart
    When I run "lns service --help"
    Then the exit code is 0
    And the output contains "restart"

  Scenario: service restart --help describes the subcommand
    When I run "lns service restart --help"
    Then the exit code is 0
    And the output contains "Usage: lns service restart"

  Scenario: service restart rejects unknown flags
    When I run "lns service restart --not-a-real-flag"
    Then the exit code is 2
    And the output contains "unexpected argument"
