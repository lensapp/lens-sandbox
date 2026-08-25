Feature: a prune prompt lands on stderr, never stdout

  §4.1: stdout carries the answer, stderr carries everything else —
  prompts included — so redirecting stdout never hides the question.
  §7.2: a prompt is written to stderr. A prune whose stdout is piped to
  a file must still show "Continue? [y/N]" on the terminal instead of
  burying it in the capture and waiting on an invisible question.

  Scenario: artifact prune asks on stderr and keeps stdout for the result
    Given two cached sandboxes and one running sandbox
    And the user will answer "y" to the sandbox prompt
    When the user runs artifact command "prune"
    Then the exit code is 0
    And the command's stderr contains "Continue? [y/N]"
    And the command's stdout does not contain "Continue?"

  Scenario: declining an artifact prune reports Aborted. on stderr
    Given two cached sandboxes and one running sandbox
    And the user will answer "n" to the sandbox prompt
    When the user runs artifact command "prune"
    Then the exit code is 0
    And the command's stderr contains "Aborted."
    And the command's stdout does not contain "Aborted."

  Scenario: sandbox prune asks on stderr and keeps stdout for the sweep report
    Given the service will sweep the stopped sandboxes "scribe" and "hermes"
    And the user will answer "y" to the sandbox prompt
    When the user runs sandbox command "prune"
    Then the exit code is 0
    And the command's stderr contains "Continue? [y/N]"
    And the command's stdout does not contain "Continue?"

  Scenario: volume prune asks on stderr and keeps stdout for the removals
    Given the volume "orphan" is held by no running sandbox and named by no cached sandbox
    And the user will answer "y" to the prompt
    When the user runs volume command "prune"
    Then the exit code is 0
    And the command's stderr contains "Continue? [y/N]"
    And the command's stdout does not contain "Continue?"
