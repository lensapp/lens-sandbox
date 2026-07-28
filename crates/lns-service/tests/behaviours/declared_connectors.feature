Feature: a sandbox definition's declared connectors seed their placeholder but stay unarmed
  A sandbox definition — `./lns.yaml` or a published sandbox artifact —
  lists under `spec.connectors` the connectors its workload would like
  to use. Declaring seeds the connector's placeholder env var so the
  workload sees the variable and attempts its first request, but it never
  arms the connector: no route is opened and no bound value is injected on
  its behalf, even for a credential already bound on this machine. Consent
  stays reactive and per-directory: the first time the workload reaches
  the connector's domain it is offered a live connect, and accepting it
  arms the connector and records the id in this directory's
  `lns-policy.yaml`. Only a connector the user has connected in this
  directory (the overlay) arms at launch. A sandbox that must have a
  credential declares a `spec.credentials` slot instead (see
  credential_at_boot.feature). Real secrets never enter the artifact or the
  workload.

  Scenario: A declared connector seeds its placeholder but is not armed, only offered
    Given the machine catalog has a credential connector "some-provider" managing "SOME_TOKEN" with a route to "api.some-provider.example"
    And the sandbox definition declares connector "some-provider"
    And the directory's lns-policy.yaml connects no connectors
    When the sandbox is launched
    Then the workload's environment contains the "SOME_TOKEN" placeholder
    And the running policy does not allow the "api.some-provider.example" route
    And "some-provider" is offered for a reactive connect

  Scenario: A published sandbox's declared connectors seed their placeholder, offered not armed
    Given the machine catalog has a credential connector "some-provider" managing "SOME_TOKEN"
    And a published sandbox artifact declares connector "some-provider"
    And the directory's lns-policy.yaml connects no connectors
    When the published sandbox is launched
    Then the workload's environment contains the "SOME_TOKEN" placeholder
    And "some-provider" is offered for a reactive connect

  Scenario: An undeclared catalog connector does not seed a phantom placeholder
    Given the machine catalog has a credential connector "some-provider" managing "SOME_TOKEN" with a route to "api.some-provider.example"
    And the machine catalog has a credential connector "other-provider" managing "OTHER_TOKEN" with a route to "api.other.example"
    And the sandbox definition declares connector "some-provider"
    And the directory's lns-policy.yaml connects no connectors
    When the sandbox is launched
    Then the workload's environment does not seed the "OTHER_TOKEN" placeholder
    And "other-provider" is offered for a reactive connect

  Scenario: A connector connected in this directory still arms at launch
    Given the machine catalog has a credential connector "some-provider" managing "SOME_TOKEN" with a route to "api.some-provider.example"
    And the sandbox definition declares connector "some-provider"
    And the directory's lns-policy.yaml connects "some-provider"
    When the sandbox is launched
    Then the workload's environment contains the "SOME_TOKEN" placeholder
    And the running policy allows the "api.some-provider.example" route

  Scenario: A connected connector's machine-stored value arms at the boundary at launch
    Given the machine catalog has a credential connector "some-provider" managing "SOME_TOKEN" with a route to "api.some-provider.example"
    And the sandbox definition declares connector "some-provider"
    And the directory's lns-policy.yaml connects "some-provider"
    And the per-machine credential store has a stored value for "some-provider"
    When the sandbox is launched
    Then the boundary injection for "some-provider" is armed with the stored value

  Scenario: A committed overlay this workload never granted does not arm the machine-stored value
    Given the machine catalog has a credential connector "some-provider" managing "SOME_TOKEN" with a route to "api.some-provider.example"
    And the sandbox definition declares connector "some-provider"
    And the directory's lns-policy.yaml connects "some-provider"
    And the per-machine credential store has a stored value for "some-provider"
    And this workload has no grant for "some-provider"
    When the sandbox is launched
    Then the workload's environment contains the "SOME_TOKEN" placeholder
    And the boundary injection for "some-provider" stays unarmed

  Scenario: A declared connector's machine-stored value stays unarmed until connected
    Given the machine catalog has a credential connector "some-provider" managing "SOME_TOKEN" with a route to "api.some-provider.example"
    And the sandbox definition declares connector "some-provider" and allows the "api.some-provider.example" route
    And the directory's lns-policy.yaml connects no connectors
    And the per-machine credential store has a stored value for "some-provider"
    When the sandbox is launched
    Then the workload's environment contains the "SOME_TOKEN" placeholder
    And the running policy allows the "api.some-provider.example" route
    And the boundary injection for "some-provider" stays unarmed
    And "some-provider" is offered for a reactive connect

  Scenario: An undeclared catalog connector's machine-stored value stays unarmed
    Given the machine catalog has a credential connector "some-provider" managing "SOME_TOKEN" with a route to "api.some-provider.example"
    And the machine catalog has a credential connector "other-provider" managing "OTHER_TOKEN" with a route to "api.other.example"
    And the sandbox definition declares connector "some-provider"
    And the directory's lns-policy.yaml connects no connectors
    And the per-machine credential store has a stored value for "other-provider"
    When the sandbox is launched
    Then the boundary injection for "other-provider" stays unarmed

  Scenario: A declared connector does not open a route past a local deny-by-default
    Given the machine catalog has a credential connector "some-provider" managing "SOME_TOKEN" with a route to "api.some-provider.example"
    And the sandbox definition declares connector "some-provider"
    And the directory's lns-policy.yaml denies all by default
    When the sandbox is launched
    Then a workload request to "api.some-provider.example" is denied by policy

  Scenario: A declared oauth connector does not block the launch; it is offered
    Given the machine catalog has an oauth connector "some-oauth"
    And the sandbox definition declares connector "some-oauth"
    And the per-machine credential store has no grant for "some-oauth"
    When the sandbox is launched
    Then the workload starts
    And "some-oauth" is offered for a reactive connect

  Scenario: Accepting a reactive connect persists to the directory policy, never the definition
    Given a launched sandbox whose definition declares connector "some-provider"
    When the developer approves a new destination "api.example.test" with "always allow"
    Then the allow rule is written to the directory's lns-policy.yaml
    And the sandbox definition is not modified

  Scenario: A credential slot does not open a route past a local deny-by-default
    Given the machine catalog has a credential connector "some-provider" managing "SOME_TOKEN" with a route to "api.some-provider.example"
    And the sandbox definition declares an optional credential slot "some-provider" injected as "SOME_TOKEN"
    And the directory's lns-policy.yaml denies all by default
    When the sandbox is launched
    Then a workload request to "api.some-provider.example" is denied by policy

  Scenario: A connectable sharing a declared connector's domain is suppressed, not offered
    Given the machine catalog has a credential connector "some-primary" managing "PRIMARY_TOKEN" with a route to "api.shared.example"
    And the machine catalog has a credential connector "some-secondary" managing "SECONDARY_TOKEN" with a route to "api.shared.example"
    And the sandbox definition declares connector "some-primary"
    And the directory's lns-policy.yaml connects no connectors
    When the sandbox is launched
    Then "some-secondary" is not offered for a reactive connect

  Scenario: An unknown declared connector refuses the launch
    Given the sandbox definition declares connector "some-unknown"
    And the machine catalog has no connector "some-unknown"
    When the sandbox is launched
    Then the launch is refused
    And the error names "some-unknown"
    And the error points at `lns connector add`
