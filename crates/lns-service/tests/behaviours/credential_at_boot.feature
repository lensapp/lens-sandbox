Feature: an agent's credential slots resolve at boot, before the workload starts
  A bundle's Agent declares credential slots — each naming an integration,
  the env var it injects into, and whether it is required. A bundle resolves
  every declared slot at boot: a bound slot is armed silently, and any
  unbound slot prompts to connect before the workload starts, so there is no
  mid-run surprise. Declining a required slot aborts the launch; declining an
  optional slot proceeds with that slot unbound. The developer is shown where
  each credential will be injected before binding a real value, so a bundle
  cannot quietly redirect a token. Real credential material never enters the
  artifact or the workload — only the boundary sees it.

  NOTE: these scenarios pin the boot-gate DECISION logic (plan_slot / boot_gate
  / resolve_connect) in isolation. The gate IS live for a sandbox definition's
  declared oauth integrations (see declared_integrations.feature — the launch
  blocks pre-boot on the sign-in). For a bundle Agent's credential slots the
  shipping flow remains reactive (an unbound credential prompts when the
  workload first uses it); wiring the gate to Agent slots is a tracked follow-up.

  Scenario: A slot already bound in the store resolves at boot without prompting
    Given a bundle whose agent declares a credential slot for integration "some-provider" injected as "SOME_TOKEN"
    And the per-machine credential store has a bound value for "some-provider"
    When the bundle is launched
    Then the slot is resolved from the store at boot
    And no credential prompt is shown
    And the workload starts

  Scenario: An unbound slot prompts to connect before the workload starts
    Given a bundle whose agent declares a credential slot for integration "some-provider" injected as "SOME_TOKEN"
    And the per-machine credential store has no value for "some-provider"
    When the bundle is launched
    Then a connect prompt for "some-provider" is shown before the workload starts
    And the workload does not start until the slot is decided

  Scenario: The injection target is disclosed before a real value is bound
    Given an unbound credential slot for integration "some-provider" injected as "SOME_TOKEN"
    When the connect prompt is shown
    Then the prompt names the injection target "SOME_TOKEN" before any real value is entered

  Scenario: Declining a required slot aborts the launch
    Given an unbound required credential slot for integration "some-provider" injected as "SOME_TOKEN"
    When the developer declines to connect "some-provider"
    Then the launch is aborted
    And the workload never starts

  Scenario: Declining an optional slot proceeds with the slot unbound
    Given an unbound optional credential slot for integration "some-provider" injected as "SOME_TOKEN"
    When the developer declines to connect "some-provider"
    Then the workload starts with "SOME_TOKEN" left unbound

  Scenario: A bound slot is injected only at the boundary, never into the artifact
    Given a bundle whose agent declares a credential slot for integration "some-provider" injected as "SOME_TOKEN"
    And the per-machine credential store has a bound value for "some-provider"
    When the bundle is launched
    Then the workload sees only a placeholder in "SOME_TOKEN"
    And the real value is substituted at the boundary