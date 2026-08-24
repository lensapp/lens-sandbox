Feature: volume lifecycle management end to end
  `lns volume` drives the service's named-volume store over the real
  Unix-socket IPC: create provisions a backing image under the cache
  home, ls and inspect report it, rm and prune delete it. The
  guest-observable mount behaviour lives in the @microvm volumes
  feature; this one pins the host-side wiring through real binaries.

  Background:
    Given a clean lns cache home
    And the Lens Sandbox service is running in that home

  Scenario: create, list, inspect, and remove round-trip
    When I run "lns volume create prism-data"
    Then the exit code is 0
    And the output contains "prism-data"
    When I run "lns volume ls"
    Then the output contains "prism-data"
    And the output contains "ON DISK"
    When I run "lns volume inspect prism-data"
    Then the output contains "CAPACITY"
    When I run "lns volume inspect prism-data --format json"
    Then the output contains "sizeBytes"
    When I run "lns volume rm prism-data"
    Then the exit code is 0
    When I run "lns volume ls"
    Then the output does not contain "prism-data"

  Scenario: a created volume image carries an internal journal
    When I run "lns volume create prism-data"
    Then the exit code is 0
    And the backing image for volume "prism-data" declares an internal journal

  Scenario: prune reclaims every idle volume
    When I run "lns volume create prism-data"
    And I run "lns volume create scratch"
    And I run "lns volume prune --force"
    Then the exit code is 0
    And the output contains "Total reclaimed space:"
    When I run "lns volume ls"
    Then the output does not contain "prism-data"
    And the output does not contain "scratch"
