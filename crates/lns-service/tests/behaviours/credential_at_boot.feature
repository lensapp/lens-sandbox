Feature: a sandbox definition's declared credentials gate the launch
  A sandbox definition declares credentials under `spec.credentials` — each one
  the whole injection contract: the variable the workload reads, the placeholder
  it holds, and the domains the real value may travel to. A declaration names no
  connector, so nothing in the artifact decides how the value is obtained. This
  machine's catalog decides that, by claiming a domain the declaration injects
  on: a connector that claims one supplies the value, and its sign-in is offered
  instead of a paste. A declaration nothing claims still works — the workload
  holds the placeholder and the first request asks for the value. Real credential
  material never enters the artifact or the workload; only the boundary sees it.

  NOTE: this contract covers the flat sandbox definition's `spec.credentials`.

  Scenario: A declaration is wired as written, not as the catalog would have it
    Given the machine catalog has a credential connector "some-provider" managing "CATALOG_DEFAULT"
    And the sandbox definition declares a credential "PROVIDER_KEY" injected on "api.example.test"
    And the per-machine credential store has a stored value for "some-provider"
    When the sandbox is launched
    Then the workload's environment contains the placeholder under "PROVIDER_KEY"
    And no value-decision prompt is shown before the workload starts

  Scenario: A declaration no connector claims is still wired, and asks at first use
    Given the machine catalog has no connector "some-provider"
    And the sandbox definition declares a credential "SOME_TOKEN" injected on "api.nobody-claims.example"
    When the sandbox is launched
    Then the workload's environment contains the placeholder under "SOME_TOKEN"
    And no value-decision prompt is shown before the workload starts

  Scenario: A host-detect decision counts as bound for the launch gate
    Given the machine catalog has a credential connector "some-provider" managing "CATALOG_DEFAULT"
    And the sandbox definition declares a credential "SOME_TOKEN" injected on "api.example.test"
    And the per-machine credential store has a host-detect entry for "some-provider"
    When the sandbox is launched
    Then the workload's environment contains the placeholder under "SOME_TOKEN"

  Scenario: A credential denied on this machine leaves the declaration unarmed rather than refusing
    Given the machine catalog has a credential connector "some-provider" managing "CATALOG_DEFAULT"
    And the sandbox definition declares a credential "SOME_TOKEN" injected on "api.example.test"
    And the per-machine credential store has a deny entry for "some-provider"
    When the sandbox is launched
    Then the workload's environment contains the placeholder under "SOME_TOKEN"
    And no value-decision prompt is shown before the workload starts

  Scenario: A bound declaration is injected only at the boundary, never into the workload
    Given the machine catalog has a credential connector "some-provider" managing "CATALOG_DEFAULT"
    And the sandbox definition declares a credential "SOME_TOKEN" injected on "api.example.test"
    And the per-machine credential store has a stored value for "some-provider"
    When the sandbox is launched
    Then the workload sees only a placeholder in "SOME_TOKEN"
    And the real value is substituted at the boundary

  Scenario: An oauth connector claiming the declared domain blocks the boot on its sign-in
    Given the machine catalog has an oauth connector "some-oauth"
    And the sandbox definition declares a credential "SOME_OAUTH_TOKEN" injected on "api.some-oauth.example"
    And the per-machine credential store has no grant for "some-oauth"
    When the sandbox is launched
    Then a sign-in prompt for "some-oauth" is shown before the workload starts
    And the workload does not start until the sign-in is decided

  Scenario: Completing the boot sign-in is this workload's grant, so the declaration arms without a second card
    Given the machine catalog has an oauth connector "some-oauth"
    And the sandbox definition declares a credential "SOME_OAUTH_TOKEN" injected on "api.some-oauth.example"
    And the per-machine credential store has no grant for "some-oauth"
    And this workload has no grant for "some-oauth"
    When the sandbox is launched
    And the boot sign-in for "some-oauth" completes
    Then the boundary injection for "some-oauth" is armed with the signed-in token
