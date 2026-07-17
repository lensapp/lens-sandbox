Feature: lns run falls back to configured defaults
  The `lns config` gap-fillers (vCPUs, memory, registry) apply to `lns run`
  whenever the matching per-run flag is absent; a per-run flag always wins.
  Env, volumes, and ports are no longer config defaults — they live in the
  sandbox definition.

  Scenario: Configured resources apply when no flag is passed
    Given the default "run.cpus" is "4"
    And the default "run.mem" is "2048"
    When the user resolves `lns run alpine` against the configured defaults
    Then the run summary shows "4 vCPU · 2048 MiB"

  Scenario: A per-run flag overrides the configured resource default
    Given the default "run.cpus" is "4"
    When the user resolves `lns run --cpus 2 alpine` against the configured defaults
    Then the run summary shows "2 vCPU · 512 MiB"

  Scenario: Built-in defaults apply when nothing is configured
    When the user resolves `lns run alpine` against the configured defaults
    Then the run summary shows "1 vCPU · 512 MiB"
