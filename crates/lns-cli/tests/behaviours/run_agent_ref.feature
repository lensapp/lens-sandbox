Feature: Running an agent reference

  `lns run <agent-ref>` resolves a typed agent artifact from the registry into a
  concrete image + command, and warns (without blocking) about credentials that
  are not yet provisioned on this machine.

  Scenario: an agent reference resolves to its image and command
    Given an agent artifact "localhost:5000/org/acme/agents/hermes:v1" with image "localhost:5000/org/acme/images/hermes-agent:v2026.6.5" and command "start hermes"
    When the developer launches "localhost:5000/org/acme/agents/hermes:v1"
    Then the resolved image is "localhost:5000/org/acme/images/hermes-agent:v2026.6.5"
    And the resolved command is "/bin/sh -c start hermes"

  Scenario: an unprovisioned credential produces a connect warning
    Given an agent artifact "localhost:5000/org/acme/agents/hermes:v1" needing credential "some-provider"
    When the developer launches "localhost:5000/org/acme/agents/hermes:v1"
    Then a credential warning names "some-provider"

  Scenario: a non-runnable family is refused
    When the developer launches "localhost:5000/org/acme/policies/pii:v1"
    Then the run is refused because the reference is not runnable
