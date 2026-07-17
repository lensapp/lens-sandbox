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

  Scenario: push --dry-run builds everything and uploads nothing
    Given a valid lns.yaml in the current directory
    When the user runs sandbox command "push --dry-run ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "would push ghcr.io/team/hermes:1.4.0@sha256:"
    And the output contains "nothing uploaded"
    And nothing is pushed

  Scenario: push --dry-run packs path filesets and reports their pinned refs
    Given a valid lns.yaml in the current directory declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains "prompts.md"
    When the user runs sandbox command "push --dry-run ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "would push fileset"
    And the output contains "@sha256:"
    And nothing is pushed

  Scenario: push --dry-run refuses an invalid definition like a real push
    Given a valid lns.yaml in the current directory declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains ".env"
    When the user runs sandbox command "push --dry-run ghcr.io/team/hermes:1.4.0"
    Then the command fails with an exit code other than 0
    And the output contains ".env"
    And nothing is pushed

  Scenario: push fails clearly when the credential lacks write scope
    Given a valid lns.yaml in the current directory
    And the stored credential for the registry lacks push scope
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the command fails with an exit code other than 0
    And the output contains "push scope"
    And the output contains "ghcr.io"

  Scenario: push packs each path fileset and pins it into the published config
    Given a valid lns.yaml in the current directory declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains "prompts.md"
    And the registry accepts the push
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And a FileSet artifact is pushed alongside the sandbox
    And the published sandbox config carries the fileset as a digest-pinned ref, not a path

  Scenario: a secret-shaped file in a path fileset refuses the push
    Given a valid lns.yaml in the current directory declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains ".env"
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the command fails with an exit code other than 0
    And the output contains ".env"
    And nothing is pushed

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
