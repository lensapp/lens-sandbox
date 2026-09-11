Feature: Inspecting a sandbox's current configuration
  Scenario: Reviewing a mixin before starting a sandbox
    When a sandbox declaring Node 20 is previewed with a Node 22 mixin
    Then the preview identifies the added mixin and the tool it replaces

  Scenario: Decisions made after launch precede connector and document rules
    Given a recorded sandbox with document rules and a connector grant
    When the sandbox configuration is inspected after a deny decision
    Then the configuration attributes the current rules in enforcement order
    And the configuration exposes connector destinations but no secret values

  Scenario: Withdrawing a decision reveals the original document rule
    Given a recorded sandbox with document rules and a connector grant
    When the sandbox configuration is inspected without a persistent decision
    Then no user decision remains in the configuration
