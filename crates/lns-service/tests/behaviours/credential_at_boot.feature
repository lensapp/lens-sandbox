Feature: a sandbox definition's credential slots gate the launch
  A sandbox definition declares credential slots under `spec.credentials` —
  each naming an integration, the env var it is injected as (a remap of the
  catalog's default), and whether the workload requires it. Every slot
  resolves against this machine's per-machine credential store at launch:
  a bound slot arms silently under the slot's env name, an unbound optional
  slot runs reactively (the first placeholder use pauses for the value
  decision, exactly as today), and an unbound required slot refuses the
  launch before any microVM boots — naming the credential, its injection
  target, and the `lns integration connect` fix. A credential denied on
  this machine refuses a required slot distinctly: "you denied this" is
  not "never bound". A required oauth-kind slot composes with the declared
  sign-in gate instead of duplicating it. Real credential material never
  enters the artifact or the workload — only the boundary sees it.

  NOTE: this contract covers the flat sandbox definition's `spec.credentials`.
  Gating a bundle Agent's credential slots through the same seams remains a
  tracked follow-up.

  Scenario: A required slot with no bound value refuses the launch before boot
    Given the machine catalog has a credential integration "some-provider" managing "SOME_TOKEN"
    And the sandbox definition requires a credential slot for "some-provider" injected as "SOME_TOKEN"
    And the per-machine credential store has no entry for "some-provider"
    When the sandbox is launched
    Then the launch is refused
    And the error names "some-provider"
    And the error names the injection target "SOME_TOKEN"
    And the error points at `lns integration connect some-provider`

  Scenario: A denied credential refuses a required slot distinctly
    Given the machine catalog has a credential integration "some-provider" managing "SOME_TOKEN"
    And the sandbox definition requires a credential slot for "some-provider" injected as "SOME_TOKEN"
    And the per-machine credential store has a deny entry for "some-provider"
    When the sandbox is launched
    Then the launch is refused
    And the error says the credential was denied on this machine
    And the error points at `lns integration connect some-provider`

  Scenario: A host-detect decision counts as bound for the launch gate
    Given the machine catalog has a credential integration "some-provider" managing "SOME_TOKEN"
    And the sandbox definition requires a credential slot for "some-provider" injected as "SOME_TOKEN"
    And the per-machine credential store has a host-detect entry for "some-provider"
    When the sandbox is launched
    Then the workload's environment contains the "SOME_TOKEN" placeholder

  Scenario: A bound slot arms silently under the slot's env name
    Given the machine catalog has a credential integration "some-provider" managing "SOME_TOKEN"
    And the sandbox definition declares a credential slot for "some-provider" injected as "PROVIDER_KEY"
    And the per-machine credential store has a stored value for "some-provider"
    When the sandbox is launched
    Then the workload's environment contains the placeholder under "PROVIDER_KEY"
    And no value-decision prompt is shown before the workload starts

  Scenario: An unbound optional slot runs reactively
    Given the machine catalog has a credential integration "some-provider" managing "SOME_TOKEN"
    And the sandbox definition declares a credential slot for "some-provider" injected as "SOME_TOKEN"
    And the per-machine credential store has no entry for "some-provider"
    When the sandbox is launched
    Then the workload's environment contains the "SOME_TOKEN" placeholder
    And no value-decision prompt is shown before the workload starts

  Scenario: A bound slot is injected only at the boundary, never into the workload
    Given the machine catalog has a credential integration "some-provider" managing "SOME_TOKEN"
    And the sandbox definition declares a credential slot for "some-provider" injected as "SOME_TOKEN"
    And the per-machine credential store has a stored value for "some-provider"
    When the sandbox is launched
    Then the workload sees only a placeholder in "SOME_TOKEN"
    And the real value is substituted at the boundary

  Scenario: A required oauth-kind slot blocks on the sign-in gate instead of refusing
    Given the machine catalog has an oauth integration "some-oauth"
    And the sandbox definition requires a credential slot for "some-oauth" injected as "SOME_OAUTH_TOKEN"
    And the per-machine credential store has no grant for "some-oauth"
    When the sandbox is launched
    Then a sign-in prompt for "some-oauth" is shown before the workload starts
    And the workload does not start until the sign-in is decided

  Scenario: A slot naming an unknown integration refuses the launch
    Given the sandbox definition requires a credential slot for "some-unknown" injected as "SOME_TOKEN"
    And the machine catalog has no integration "some-unknown"
    When the sandbox is launched
    Then the launch is refused
    And the error names "some-unknown"
    And the error points at `lns integration add`
