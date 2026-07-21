Feature: a sandbox definition's declared integrations are offered, not armed
  A sandbox definition — `./lns.yaml` or a published sandbox artifact —
  lists under `spec.integrations` the integrations its workload would like
  to use. Declaring is disclosure, not arming: a declared id is surfaced at
  launch but never force-armed — no placeholder is seeded and no route is
  opened on its behalf, even for a credential already bound on this machine.
  Consent stays reactive and per-directory: the first time the workload
  reaches the integration's domain it is offered a live connect, and
  accepting it arms the integration and records the id in this directory's
  `lns-policy.yaml`. Only an integration the user has connected in this
  directory (the overlay) arms at launch. A sandbox that must have a
  credential declares a `spec.credentials` slot instead (see
  credential_at_boot.feature). Real secrets never enter the artifact or the
  workload.

  Scenario: A declared integration is not armed at launch, only offered
    Given the machine catalog has a credential integration "some-provider" managing "SOME_TOKEN" with a route to "api.some-provider.example"
    And the sandbox definition declares integration "some-provider"
    And the directory's lns-policy.yaml connects no integrations
    When the sandbox is launched
    Then the workload's environment does not seed the "SOME_TOKEN" placeholder
    And the running policy does not allow the "api.some-provider.example" route
    And "some-provider" is offered for a reactive connect

  Scenario: A published sandbox's declared integrations are offered, not armed
    Given the machine catalog has a credential integration "some-provider" managing "SOME_TOKEN"
    And a published sandbox artifact declares integration "some-provider"
    And the directory's lns-policy.yaml connects no integrations
    When the published sandbox is launched
    Then the workload's environment does not seed the "SOME_TOKEN" placeholder
    And "some-provider" is offered for a reactive connect

  Scenario: An integration connected in this directory still arms at launch
    Given the machine catalog has a credential integration "some-provider" managing "SOME_TOKEN" with a route to "api.some-provider.example"
    And the sandbox definition declares integration "some-provider"
    And the directory's lns-policy.yaml connects "some-provider"
    When the sandbox is launched
    Then the workload's environment contains the "SOME_TOKEN" placeholder
    And the running policy allows the "api.some-provider.example" route

  Scenario: A declared integration does not open a route past a local deny-by-default
    Given the machine catalog has a credential integration "some-provider" managing "SOME_TOKEN" with a route to "api.some-provider.example"
    And the sandbox definition declares integration "some-provider"
    And the directory's lns-policy.yaml denies all by default
    When the sandbox is launched
    Then a workload request to "api.some-provider.example" is denied by policy

  Scenario: A declared oauth integration does not block the launch; it is offered
    Given the machine catalog has an oauth integration "some-oauth"
    And the sandbox definition declares integration "some-oauth"
    And the per-machine credential store has no grant for "some-oauth"
    When the sandbox is launched
    Then the workload starts
    And "some-oauth" is offered for a reactive connect

  Scenario: Accepting a reactive connect persists to the directory policy, never the definition
    Given a launched sandbox whose definition declares integration "some-provider"
    When the developer approves a new destination "api.example.test" with "always allow"
    Then the allow rule is written to the directory's lns-policy.yaml
    And the sandbox definition is not modified

  Scenario: A credential slot does not open a route past a local deny-by-default
    Given the machine catalog has a credential integration "some-provider" managing "SOME_TOKEN" with a route to "api.some-provider.example"
    And the sandbox definition declares an optional credential slot "some-provider" injected as "SOME_TOKEN"
    And the directory's lns-policy.yaml denies all by default
    When the sandbox is launched
    Then a workload request to "api.some-provider.example" is denied by policy

  Scenario: A connectable sharing a declared integration's domain is suppressed, not offered
    Given the machine catalog has a credential integration "some-primary" managing "PRIMARY_TOKEN" with a route to "api.shared.example"
    And the machine catalog has a credential integration "some-secondary" managing "SECONDARY_TOKEN" with a route to "api.shared.example"
    And the sandbox definition declares integration "some-primary"
    And the directory's lns-policy.yaml connects no integrations
    When the sandbox is launched
    Then "some-secondary" is not offered for a reactive connect

  Scenario: An unknown declared integration refuses the launch
    Given the sandbox definition declares integration "some-unknown"
    And the machine catalog has no integration "some-unknown"
    When the sandbox is launched
    Then the launch is refused
    And the error names "some-unknown"
    And the error points at `lns integration add`
