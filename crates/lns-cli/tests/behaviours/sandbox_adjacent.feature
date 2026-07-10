Feature: adjacent commands reshaped around the sandbox
  The own-top-level groups keep their mechanisms but are reframed around the
  single sandbox noun: integration connect only binds credentials, config
  keeps only the run gap-fillers, volumes stay durable and never swept by
  sandbox GC, and the audit chain records a sandbox_run.

  @todo
  Scenario: integration connect binds a credential and does not declare
    Given the integration "some-provider" is in the catalog
    When the user runs integration command "connect some-provider"
    Then the exit code is 0
    And the output describes binding a credential value
    And the output does not claim to add the integration to a sandbox

  Scenario: config set accepts only the run gap-filler keys
    When the user runs config command "set run.registry ghcr.io"
    Then the exit code is 0
    When the user runs config command "set run.cpus 4"
    Then the exit code is 0
    When the user runs config command "set run.mem 2048"
    Then the exit code is 0

  Scenario Outline: config set rejects a dropped key
    When the user runs config command "set <key> <value>"
    Then the command fails with an exit code other than 0
    And the output contains "unknown key"
    Examples:
      | key         | value        |
      | run.env     | FOO=bar      |
      | run.volume  | v:/data      |
      | run.publish | 8080:80      |

  Scenario: a hand-edited legacy config key warns and is ignored
    Given a config file that still carries a run.env entry
    When the user runs config command "list"
    Then the exit code is 0
    And the output warns that "run.env" is no longer supported
    And the output does not list "run.env" as an active default

  @todo
  Scenario: volume rm refuses a volume a running sandbox holds
    Given the volume "claude-home" is held by a running sandbox
    When the user runs volume command "rm claude-home"
    Then the command fails with an exit code other than 0
    And the output contains "running"

  @todo
  Scenario: volume prune removes only unreferenced volumes and confirms first
    Given the volume "orphan" is held by no running sandbox and named by no cached sandbox
    And the volume "claude-home" is named by a cached sandbox
    When the user runs volume command "prune --force"
    Then the exit code is 0
    And the volume "orphan" is removed
    And the volume "claude-home" is kept

  @todo
  Scenario: audit records a sandbox_run event
    Given the service has recorded a sandbox run in the audit chain
    When the user runs "lns audit --json"
    Then the exit code is 0
    And the output contains "sandbox_run"
    And the output does not contain "bundle_run"

  @todo
  Scenario: audit scopes to one sandbox by name
    Given the audit chain carries events for sandboxes "hermes" and "scribe"
    When the user runs "lns audit hermes"
    Then the exit code is 0
    And the output contains "hermes"
    And the output does not contain "scribe"
