Feature: image cache management end to end
  `lns image` drives the service's pulled-image cache over the real
  Unix-socket IPC. Pulling needs registry network access, so this
  feature pins the host-side wiring of the offline verbs through real
  binaries: ls renders the cache table, rm refuses unknown images, and
  prune reports a clean cache.

  Background:
    Given a clean lns cache home
    And the Lens Sandbox service is running in that home

  Scenario: listing an empty cache renders only the table header
    When I run "lns image ls"
    Then the exit code is 0
    And the output contains "REFERENCE"
    And the output contains "DIGEST"
    And the output contains "IN USE"

  Scenario: removing an image that is not cached fails cleanly
    When I run "lns image rm registry.example.test/absent:1"
    Then the exit code is non-zero
    And the output contains "no such image"

  Scenario: pruning an empty cache reports nothing to remove
    When I run "lns image prune --force"
    Then the exit code is 0
    And the output contains "No unused images."
