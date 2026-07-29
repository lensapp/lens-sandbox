Feature: consent to publisher-declared tool installation
  Pulling a published sandbox may execute its declared third-party installers
  in the provisioning microVM, so the consumer sees and accepts that effect
  before the service begins the pull and provisioning operation.

  Background:
    Given the registry serves the sandbox "ghcr.io/team/hermes:1.4.0"
    And the published sandbox declares tool "node@22"

  Scenario: an interactive pull discloses tools before provisioning
    Given the user will answer "yes" to the sandbox prompt
    When the user runs sandbox command "pull ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "Tool:       node@22"
    And the output contains "installer runs as root in a disposable microVM"
    And the pull request is bound to the inspected digest

  Scenario: declining sends no provisioning request
    Given the user will answer "no" to the sandbox prompt
    When the user runs sandbox command "pull ghcr.io/team/hermes:1.4.0"
    Then the exit code is 1
    And the output contains "declined"
    And the service received no pull request

  Scenario: a non-interactive pull fails closed
    Given sandbox input is non-interactive
    When the user runs sandbox command "pull ghcr.io/team/hermes:1.4.0"
    Then the exit code is 1
    And the output contains "--yes"
    And the service received no pull request

  Scenario: --yes accepts declared tools without prompting
    Given sandbox input is non-interactive
    When the user runs sandbox command "pull --yes ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output does not contain "Continue?"
    And the pull request is bound to the inspected digest
