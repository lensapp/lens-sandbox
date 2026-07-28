Feature: users discover and parse the service login-agent subcommands
  `lns service enable` registers a per-user login agent so the sandbox
  starts now and on every login; `lns service disable` stops it and
  removes the login agent. Both must be discoverable from the service
  help and parse cleanly. Behavioural depth (rendering, idempotency,
  graceful degradation) is pinned by Layer 3 unit tests, because the
  Layer 2 harness only parses argv — it never dispatches to launchctl
  or systemctl.

  Scenario: service help lists enable and disable
    When I run "lns service --help"
    Then the exit code is 0
    And the output contains "enable"
    And the output contains "disable"

  Scenario: service enable --help describes the login-agent registration
    When I run "lns service enable --help"
    Then the exit code is 0
    And the output contains "Usage: lns service enable"
    And the output contains "every login"

  Scenario: service disable --help describes unregistering the login agent
    When I run "lns service disable --help"
    Then the exit code is 0
    And the output contains "Usage: lns service disable"
    And the output contains "login agent"

  Scenario: service status --help offers the machine-readable format
    When I run "lns service status --help"
    Then the exit code is 0
    And the output contains "--format"
    And the output contains "experimental"
