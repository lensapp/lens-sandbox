@microvm
Feature: Docker-compat run flags that need a real image
  These boot a pulled alpine image, so unlike the imageless compat-flags
  scenarios they reach the network (Docker Hub) on a cold cache; like all
  microVM image work they run only via `make e2e-microvm`, never in CI.
  They cover the flag behaviours an imageless run can't exercise: a command
  after the image with no `--` separator, an `--entrypoint` override, and a
  named `-u`/`--user` resolved against the image's `/etc/passwd`. Markers are
  shell-computed so they match only real workload output, not the command the
  supervisor echoes in its `[agent] starting:` line.

  Scenario: a command after the image runs without an explicit separator
    Given the Lens Sandbox service is running
    When the user runs image "alpine:3.20" with command "/bin/sh -c 'echo nodash-$((6*7))'" and no separator
    Then the exit code is 0
    And the output contains "nodash-42"

  Scenario: --entrypoint sets the program the workload runs
    Given the Lens Sandbox service is running
    When the user runs image "alpine:3.20" with entrypoint "/bin/sh" and command "-c 'echo entry-$((7*8))'"
    Then the exit code is 0
    And the output contains "entry-56"

  Scenario: -u/--user resolves a named user against the image
    Given the Lens Sandbox service is running
    When the user runs image "alpine:3.20" as user "games" with command "id"
    Then the exit code is 0
    And the output contains "uid=35(games)"
