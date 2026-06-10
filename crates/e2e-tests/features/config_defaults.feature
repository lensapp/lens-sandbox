Feature: lns config persists defaults across invocations
  Wiring confirmation through the real binary: `lns config` writes a
  per-user config file that later invocations read back, and `lns run`
  consults the same file before any service round-trip.

  Scenario: A default set by one invocation is visible to the next
    Given a clean lns cache home
    When I run "lns config set run.cpus 4"
    Then the exit code is 0
    And the output contains "Set run.cpus to 4"
    When I run "lns config get run.cpus"
    Then the exit code is 0
    And the output contains "4"

  Scenario: Listing from a fresh home shows no defaults
    Given a clean lns cache home
    When I run "lns config list"
    Then the exit code is 0
    And the output contains "No defaults set in"

  Scenario: lns run rejects a malformed hand-edited default before any service round-trip
    Given a clean lns cache home
    And the home config file declares a malformed run.env entry "BARE"
    When I run "lns run alpine"
    Then the exit code is non-zero
    And the output contains "run.env"
