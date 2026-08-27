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

  Scenario: A bare mixin reference is qualified with the built-in default registry
    When the user resolves `lns run --mixin acme/obs-tools:2 alpine` against the configured defaults
    Then the run carries the mixin "hub.lns.run/acme/obs-tools:2"

  Scenario: A configured registry qualifies a bare mixin reference
    Given the default "run.registry" is "ghcr.io"
    When the user resolves `lns run --mixin acme/obs-tools:2 alpine` against the configured defaults
    Then the run carries the mixin "ghcr.io/acme/obs-tools:2"

  Scenario: A fully-qualified mixin reference is used as the user typed it
    When the user resolves `lns run --mixin ghcr.io/acme/obs-tools:2 alpine` against the configured defaults
    Then the run carries the mixin "ghcr.io/acme/obs-tools:2"

  Scenario: A mixin named by a path is a local document, never a reference
    When the user resolves `lns run --mixin ./obs alpine` against the configured defaults
    Then the run carries the mixin "./obs"

  Scenario: The resources a document declares outrank a configured default
    Given the default "run.cpus" is "4"
    And the default "run.mem" is "2048"
    And an lns.yaml declaring 3 vCPU and 6Gi of memory
    When the local run summary is composed against the configured defaults
    Then the run summary shows "3 vCPU · 6144 MiB"

  Scenario: A per-run flag outranks both the document and a configured default
    Given the default "run.cpus" is "4"
    And an lns.yaml declaring 3 vCPU and 6Gi of memory
    When the local run summary is composed against the configured defaults with "--cpus 8"
    Then the run summary shows "8 vCPU · 6144 MiB"

  Scenario: A configured default fills what the document leaves unsaid
    Given the default "run.cpus" is "4"
    And an lns.yaml declaring a 40Gi disk
    When the local run summary is composed against the configured defaults
    Then the run summary shows "4 vCPU · 512 MiB · 40 GiB disk"
