Feature: a sandbox definition's declared integrations arm at launch
  A sandbox definition — `./lns.yaml` or a published sandbox artifact —
  declares the integrations its workload needs under `spec.integrations`.
  Declaring is disclosure and arming in one: at launch the declared ids
  resolve against the machine's integration catalog, their placeholders
  are seeded, and their routes are allowed for the run, with no
  per-directory `lns integration connect` step. The per-machine value
  decision remains the consent gate — the first request carrying a
  placeholder still pauses for approval, and real secrets never enter
  the artifact or the workload. `lns integration connect` keeps working:
  it stays the sign-in vehicle for oauth integrations and the way to arm
  a directory that has no definition.

  Scenario: A definition-declared integration is armed at launch without a local connect
    Given the machine catalog has a credential integration "some-provider" managing "SOME_TOKEN" with a route to "api.some-provider.example"
    And the sandbox definition declares integration "some-provider"
    And the directory's lns-policy.yaml connects no integrations
    When the sandbox is launched
    Then the workload's environment contains the "SOME_TOKEN" placeholder
    And the running policy allows the "api.some-provider.example" route

  Scenario: A published sandbox's declared integrations arm on the consuming machine
    Given the machine catalog has a credential integration "some-provider" managing "SOME_TOKEN"
    And a published sandbox artifact declares integration "some-provider"
    And the directory's lns-policy.yaml connects no integrations
    When the published sandbox is launched
    Then the workload's environment contains the "SOME_TOKEN" placeholder

  @todo
  Scenario: First placeholder use still pauses for the per-machine value decision
    Given a launched sandbox whose definition declares integration "some-provider"
    And no credential rule exists in "~/.lns-credentials.json" for "some-provider"
    When the workload sends a request carrying the some-provider placeholder
    Then a credential card appears for "some-provider"
    And the workload's request is held pending a decision

  Scenario: Declared and connected integrations union without duplication
    Given the machine catalog has credential integrations "some-provider" and "other-provider"
    And the sandbox definition declares integrations "some-provider" and "other-provider"
    And the directory's lns-policy.yaml connects "some-provider"
    When the sandbox is launched
    Then the workload's environment seeds "some-provider" and "other-provider" each exactly once

  @todo
  Scenario: A local deny still dominates a declared integration's route
    Given the machine catalog has a credential integration "some-provider" managing "SOME_TOKEN" with a route to "api.some-provider.example"
    And the sandbox definition declares integration "some-provider"
    And the directory's lns-policy.yaml denies "api.some-provider.example"
    When the sandbox is launched
    Then a workload request to "api.some-provider.example" is denied by policy

  @todo
  Scenario: Approvals persist to the directory policy, never to the definition
    Given a launched sandbox whose definition declares integration "some-provider"
    When the developer approves a new destination "api.example.test" with "always allow"
    Then the allow rule is written to the directory's lns-policy.yaml
    And the sandbox definition is not modified

  Scenario: An unknown declared integration refuses the launch
    Given the sandbox definition declares integration "some-unknown"
    And the machine catalog has no integration "some-unknown"
    When the sandbox is launched
    Then the launch is refused
    And the error names "some-unknown"
    And the error points at `lns integration add`

  @todo
  Scenario: A declared oauth integration with no machine grant blocks the launch
    Given the machine catalog has an oauth integration "some-oauth"
    And the sandbox definition declares integration "some-oauth"
    And the per-machine credential store has no grant for "some-oauth"
    When the sandbox is launched
    Then a sign-in prompt for "some-oauth" is shown before the workload starts
    And the workload does not start until the sign-in is decided

  @todo
  Scenario: Completing the sign-in releases the blocked launch
    Given a launch blocked on the "some-oauth" sign-in
    When the sign-in completes
    Then the workload starts with the "some-oauth" placeholder seeded

  @todo
  Scenario: Declining the sign-in aborts the launch
    Given a launch blocked on the "some-oauth" sign-in
    When the developer declines the sign-in
    Then the launch is aborted
    And the workload never starts
