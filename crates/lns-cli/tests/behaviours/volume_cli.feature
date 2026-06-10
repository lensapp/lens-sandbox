Feature: managing named volumes from the CLI
  `lns volume` is the lifecycle surface for the named volumes that
  `lns run -v name:/path` mounts: list them, provision one ahead of its
  first run, inspect one as JSON, and reclaim space with rm/prune. The
  store lives in the service, so every verb is a thin IPC call.

  Scenario: the volume family lists its verbs in help
    When I run "lns volume --help"
    Then the exit code is 0
    And the output contains "ls"
    And the output contains "create"
    And the output contains "inspect"
    And the output contains "rm"
    And the output contains "prune"

  Scenario: a bare volume invocation surfaces usage
    When I run "lns volume"
    Then the exit code is 2
    And the output contains "Usage: lns volume"

  Scenario: an invalid volume name is rejected before any IPC
    When I run "lns volume create ../etc"
    Then the exit code is 2
    And the output contains "invalid volume name"

  Scenario: listing volumes renders a table with size and holder
    Given the service reports a volume "prism-data" of 33554432 bytes held by run 7
    When the user runs volume command "ls"
    Then the exit code is 0
    And the output contains "NAME"
    And the output contains "prism-data"
    And the output contains "32 MiB"
    And the output contains "run #7"

  Scenario: listing volumes marks an unattached volume as idle
    Given the service reports an idle volume "prism-data" of 33554432 bytes
    When the user runs volume command "ls"
    Then the exit code is 0
    And the listed row for "prism-data" ends with "-"

  Scenario: creating a volume confirms it by name
    When the user runs volume command "create prism-data"
    Then the exit code is 0
    And the output contains "prism-data"

  Scenario: inspecting a volume prints its details as JSON
    Given the service reports an idle volume "prism-data" of 33554432 bytes
    When the user runs volume command "inspect prism-data"
    Then the exit code is 0
    And the output is JSON describing the idle volume "prism-data" of 33554432 bytes

  Scenario: removing an idle volume confirms it by name
    When the user runs volume command "rm prism-data"
    Then the exit code is 0
    And the output contains "prism-data"

  Scenario: removing a held volume surfaces the service's refusal
    Given the service refuses with "volume \"prism-data\" in use by run #7"
    When the user runs volume command "rm prism-data"
    Then the exit code is 1
    And the output contains "in use by run #7"

  Scenario: pruning with --force skips the prompt and reports reclaimed space
    Given the service will prune volumes "prism-data" and "scratch" reclaiming 67108864 bytes
    When the user runs volume command "prune --force"
    Then the exit code is 0
    And the output contains "prism-data"
    And the output contains "scratch"
    And the output contains "Total reclaimed space: 64 MiB"

  Scenario: pruning prompts for confirmation and proceeds on yes
    Given the service will prune volumes "prism-data" and "scratch" reclaiming 67108864 bytes
    And the user will answer "y" to the prompt
    When the user runs volume command "prune"
    Then the exit code is 0
    And the output contains "Continue? [y/N]"
    And the output contains "Total reclaimed space: 64 MiB"

  Scenario: declining the prune prompt aborts without touching the service
    Given the user will answer "n" to the prompt
    When the user runs volume command "prune"
    Then the exit code is 0
    And the output contains "Aborted."
    And no request reached the service

  Scenario: pruning with nothing to remove says so
    Given the service will prune no volumes
    When the user runs volume command "prune --force"
    Then the exit code is 0
    And the output contains "No unused volumes."

  Scenario: an unreachable service is reported plainly
    Given the service is unreachable
    When the user runs volume command "ls"
    Then the exit code is 1
    And the output contains "no response from lns-service"
