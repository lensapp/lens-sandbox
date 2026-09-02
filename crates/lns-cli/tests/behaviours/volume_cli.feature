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
    And the output does not contain "docker" in any casing

  Scenario: a bare volume invocation surfaces usage
    When I run "lns volume"
    Then the exit code is 2
    And the output contains "Usage: lns volume"

  Scenario: an invalid volume name is rejected before any IPC
    When I run "lns volume create ../etc"
    Then the exit code is 2
    And the output contains "invalid volume name"

  Scenario: listing volumes renders a table with disk usage and holder
    Given the service reports a volume "prism-data" using 33554432 bytes on disk held by "reviewer"
    When the user runs volume command "ls"
    Then the exit code is 0
    And the output contains "NAME"
    And the output contains "ON DISK"
    And the output contains "prism-data"
    And the output contains "32 MiB"
    And the output contains "reviewer"

  Scenario: listing volumes names every sandbox holding one
    Given the service reports a volume "prism-data" using 33554432 bytes on disk held by "reviewer" and "auditor"
    When the user runs volume command "ls"
    Then the exit code is 0
    And the output contains "reviewer, auditor"

  Scenario: listing volumes marks an unattached volume as idle
    Given the service reports an idle volume "prism-data" using 33554432 bytes on disk
    When the user runs volume command "ls"
    Then the exit code is 0
    And the listed row for "prism-data" ends with "-"

  Scenario: with no volumes to list, ls says so instead of printing a bare header
    Given the service reports no volumes
    When the user runs volume command "ls"
    Then the exit code is 0
    And the output contains "No volumes."
    And the output does not contain "ON DISK"

  Scenario: creating a volume confirms it by name
    When the user runs volume command "create prism-data"
    Then the exit code is 0
    And the output contains "prism-data"

  Scenario: inspecting a volume names its capacity, its on-disk size, and its holder
    Given the service reports an idle volume "prism-data" using 33554432 bytes on disk
    When the user runs volume command "inspect prism-data"
    Then the exit code is 0
    And the output contains "CAPACITY"
    And the output contains "ON DISK"
    And the output contains "32 MiB"

  Scenario: inspecting a volume as JSON gives a script the raw byte counts
    Given the service reports an idle volume "prism-data" using 33554432 bytes on disk
    When the user runs volume command "inspect prism-data --format json"
    Then the exit code is 0
    And the output is JSON describing the idle volume "prism-data" using 33554432 bytes on disk

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
    Given the volume "orphan" is held by no running sandbox and named by no cached sandbox
    And the user will answer "n" to the prompt
    When the user runs volume command "prune"
    Then the exit code is 0
    And the output contains "Aborted."
    And no prune request reached the service

  Scenario: with no terminal to ask at, prune refuses rather than assuming
    Given the volume "orphan" is held by no running sandbox and named by no cached sandbox
    And there is no terminal to ask at
    When the user runs volume command "prune"
    Then the command fails with an exit code other than 0
    And the output contains "--force"
    And no request reached the service

  Scenario: a prune whose stdin is a pipe is still asked at the terminal
    Given the volume "orphan" is held by no running sandbox and named by no cached sandbox
    And stdin is a pipe carrying "y"
    And the user will answer "n" to the prompt
    When the user runs volume command "prune"
    Then the exit code is 0
    And the output contains "Continue? [y/N]"
    And the output contains "Aborted."
    And no prune request reached the service

  Scenario: pruning with nothing to remove says so
    Given the service will prune no volumes
    When the user runs volume command "prune --force"
    Then the exit code is 0
    And the output contains "No unused volumes."

  Scenario: pruning reports volumes it could not remove and fails
    Given the service will prune volumes "prism-data" and "scratch" reclaiming 67108864 bytes
    And the service will fail to prune "stuck" with "permission denied"
    When the user runs volume command "prune --force"
    Then the exit code is 1
    And the output contains "Total reclaimed space: 64 MiB"
    And the output contains "Failed to remove stuck: permission denied"

  Scenario: an unreachable service is reported plainly
    Given the service is unreachable
    When the user runs volume command "ls"
    Then the exit code is 1
    And the output contains "no response from lns-service"
