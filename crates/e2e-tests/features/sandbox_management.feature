Feature: cached sandbox management end to end
  `lns artifact` drives the service's cache over the real
  Unix-socket IPC. Pulling needs registry network access, so this
  feature pins the host-side wiring of the offline verbs through real
  binaries: ls renders the cache table, rm refuses unknown sandboxes, and
  prune reports a clean cache.

  Background:
    Given a clean lns cache home
    And the Lens Sandbox service is running in that home

  Scenario: listing an empty cache renders only the table header
    When I run "lns artifact ls"
    Then the exit code is 0
    And the output contains "ARTIFACT"
    And the output contains "KIND"
    And the output contains "DIGEST"
    And the output contains "SIZE"
    And the output contains "HOLDER"

  Scenario: removing a sandbox that is not cached fails cleanly
    When I run "lns artifact rm registry.example.test/absent:1"
    Then the exit code is non-zero
    And the output contains "no such image"

  Scenario: pruning an empty cache reclaims nothing
    When I run "lns artifact prune --force"
    Then the exit code is 0
    And the output contains "reclaimed 0 B"

  Scenario: with no terminal to ask at, prune refuses rather than assuming
    When I run "lns artifact prune"
    Then the exit code is non-zero
    And the output contains "pass --force to confirm"
