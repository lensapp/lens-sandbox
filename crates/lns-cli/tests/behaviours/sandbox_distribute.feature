Feature: distributing a sandbox
  A sandbox is published to and pulled from an OCI registry as a typed
  artifact. Push builds then uploads in one step; there is no standalone
  build. Pull hands the reference to the service, which caches the
  artifact and prefetches its base image (pinned in lns-service).

  Scenario: push builds then uploads in a single step
    Given a valid lns.yaml in the current directory
    And the registry accepts the push
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "built"
    And the output contains "ghcr.io/team/hermes:1.4.0"

  Scenario: there is no standalone build command
    When I run "lns build ."
    Then the exit code is 2
    And the output contains "unrecognized subcommand"

  Scenario: push fails clearly when the credential lacks write scope
    Given a valid lns.yaml in the current directory
    And the stored credential for the registry lacks push scope
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the command fails with an exit code other than 0
    And the output contains "push scope"
    And the output contains "ghcr.io"

  Scenario: pull hands the reference to the service and reports the digest
    Given the registry serves the sandbox "ghcr.io/team/hermes:1.4.0"
    When the user runs sandbox command "pull ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "sha256:"
    And the service received a request to pull "ghcr.io/team/hermes:1.4.0"

  Scenario: tag re-refs a cached sandbox
    Given the sandbox "hermes:1.4.0" is cached
    When the user runs sandbox command "tag hermes:1.4.0 hermes:latest"
    Then the exit code is 0
    And the sandbox "hermes:latest" resolves to the same cached artifact
