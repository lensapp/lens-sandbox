Feature: a prune lists what it would remove before asking

  "Lists them and asks, unless -f/--force" — the question is only
  answerable when the candidates are on screen. The list rides with the
  prompt on stderr, so stdout stays reserved for what actually happened.

  Scenario: artifact prune lists the prune candidates before asking
    Given two cached sandboxes and one running sandbox
    And the user will answer "y" to the sandbox prompt
    When the user runs artifact command "prune"
    Then the exit code is 0
    And the command's stderr contains "hermes:1.0"
    And the command's stderr contains "scribe:1.0"
    And the command's stderr shows "Would remove:" before "Continue? [y/N]"
    And the service received a PruneImages request

  Scenario: an artifact prune with nothing removable still asks, for the tool cache
    Given every cached artifact is held by a running sandbox
    And the user will answer "y" to the sandbox prompt
    When the user runs artifact command "prune"
    Then the exit code is 0
    And the command's stderr does not contain "Would remove:"
    And the command's stderr contains "Continue? [y/N]"
    And the service received a PruneImages request

  Scenario: forcing an artifact prune skips the listing with the question
    Given two cached sandboxes and one running sandbox
    When the user runs artifact command "prune --force"
    Then the exit code is 0
    And the service received no ListPrunableImages request

  Scenario: sandbox prune lists the stopped sandboxes before asking
    Given the service reports one running sandbox and one that stopped
    And the user will answer "y" to the sandbox prompt
    When the user runs sandbox command "prune"
    Then the exit code is 0
    And the command's stderr contains "scribe"
    And the command's stderr does not contain "reviewer"
    And the command's stderr shows "Would remove:" before "Continue? [y/N]"

  Scenario: a sandbox prune with nothing stopped never asks
    Given the service reports one running sandbox and none stopped
    And sandbox input is a terminal
    When the user runs sandbox command "prune"
    Then the exit code is 0
    And the output contains "No stopped sandboxes."
    And the command's stderr does not contain "Continue?"
    And the service received no PruneRuns request

  Scenario: volume prune lists the unused volumes before asking
    Given the service reports a volume "prism-data" using 1024 bytes on disk held by run 7
    And the volume "orphan" is held by no running sandbox and named by no cached sandbox
    And the user will answer "y" to the prompt
    When the user runs volume command "prune"
    Then the exit code is 0
    And the command's stderr contains "orphan"
    And the command's stderr does not contain "prism-data"
    And the command's stderr shows "Would remove:" before "Continue? [y/N]"

  Scenario: a volume prune with nothing unused never asks
    Given the service reports a volume "prism-data" using 1024 bytes on disk held by run 7
    And volume input is a terminal
    When the user runs volume command "prune"
    Then the exit code is 0
    And the output contains "No unused volumes."
    And the command's stderr does not contain "Continue?"
    And no prune request reached the service
