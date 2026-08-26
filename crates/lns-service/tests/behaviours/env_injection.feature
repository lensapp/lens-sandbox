Feature: lns-service injects user env into the workload
  `lns run -e KEY=VALUE` carries non-secret configuration into the
  workload, the same role `docker run -e` plays. The service merges the
  user-supplied variables into the environment it hands the guest,
  layered so user values override the image's baked-in ENV. Injected
  variables are recorded in the run's audit entry.

  Scenario: A single env var reaches the workload
    When the user runs `lns run -e CLAUDE_CODE_USE_BEDROCK=1 someimage`
    Then the workload's environment contains CLAUDE_CODE_USE_BEDROCK set to "1"

  Scenario: -e is repeatable
    When the user runs `lns run -e A=1 -e B=2 someimage`
    Then the workload's environment contains A=1 and B=2

  Scenario: Value may contain '=' — split on the first only
    When the user runs `lns run -e DSN=user=admin;pw=x someimage`
    Then the workload's environment contains DSN set to "user=admin;pw=x"

  Scenario: Empty value is allowed
    When the user runs `lns run -e FEATURE_X= someimage`
    Then the workload's environment contains FEATURE_X set to ""

  Scenario: -e overrides a variable baked into the image
    Given the image declares ENV PORT=3003
    When the user runs `lns run -e PORT=4000 someimage`
    Then the workload's environment contains PORT set to "4000"

  Scenario: A policy-less run still injects -e (no supervisor required)
    When the user runs `lns run -e A=1 someimage`
    Then the workload's environment contains A set to "1"

  Scenario: Audit records the injected env var names but redacts their values
    When the user runs `lns run -e CLAUDE_CODE_USE_BEDROCK=1 someimage`
    Then the audit entry for the run records CLAUDE_CODE_USE_BEDROCK set to "<redacted>"
