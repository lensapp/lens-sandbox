Feature: adjacent commands reshaped around the sandbox
  The own-top-level groups keep their mechanisms but are reframed around the
  single sandbox noun: config keeps only the run gap-fillers, and volumes
  stay durable and never swept by sandbox GC. (The audit chain labels a run `sandbox_run`, pinned at Layer 3
  in audit/mod.rs, since the audit CLI reads the on-disk chain.)
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

  Scenario: a hand-edited legacy config key is ignored, and warns off stdout
    Given a config file that still carries a run.env entry
    When the user runs config command "list"
    Then the exit code is 0
    And the output does not list "run.env" as an active default
    And the output does not contain "no longer supported"

  Scenario: volume rm refuses a volume a running sandbox holds
    Given the volume "claude-home" is held by a running sandbox
    When the user runs volume command "rm claude-home"
    Then the command fails with an exit code other than 0
    And the output contains "running"

  Scenario: volume prune removes only unreferenced volumes and confirms first
    Given the volume "orphan" is held by no running sandbox and named by no cached sandbox
    And the volume "claude-home" is named by a cached sandbox
    When the user runs volume command "prune --force"
    Then the exit code is 0
    And the volume "orphan" is removed
    And the volume "claude-home" is kept
