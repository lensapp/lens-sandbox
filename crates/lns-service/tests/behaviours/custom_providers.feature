Feature: lns-service honours custom credential providers from the policy
  A custom provider declared in `lns-policy.yaml` is unioned with the
  built-in registry when a sandbox starts: its placeholder is seeded
  into the workload's environment alongside the built-ins, and a request
  carrying that placeholder drives the same credential flow as a
  built-in. The union is fixed at run start — the workload's environment
  is seeded once at boot, so a provider declared mid-run only takes
  effect for sandboxes launched afterward. A custom provider may declare
  several per-domain injections; each declared domain is intercepted at
  the boundary.

  Scenario: A sandbox launched after the declaration seeds the custom placeholder
    Given "lns-policy.yaml" declares the "acme" custom provider with env var "ACME_API_KEY"
    When a sandbox is launched against that policy file
    Then the workload's environment contains "ACME_API_KEY" set to the declared acme placeholder
    And the built-in placeholders for openai, anthropic, linear, and telegram are still present

  Scenario: A workload using the custom placeholder triggers the credential flow
    Given a sandbox is running with the "acme" custom provider declared
    And no credential rule exists for "acme"
    When the workload sends a request carrying the acme placeholder
    Then a credential card appears for "acme"
    And the workload's request is held pending a decision

  Scenario: A custom provider declared mid-run does not reach the running workload
    Given a sandbox is running with the "acme" custom provider not declared
    When the developer declares the "acme" custom provider in the loaded policy file
    Then the running workload's environment does not contain "ACME_API_KEY"
    And a new sandbox launched against the same policy file contains "ACME_API_KEY" set to the acme placeholder

  Scenario: A multi-domain custom provider expands to one boundary injection per declared domain
    Given a sandbox is running with the "acme" custom provider declaring injections for "api.acme.corp" and "api-eu.acme.corp"
    Then the seeded credentials declare a boundary injection for each of "api.acme.corp" and "api-eu.acme.corp"
