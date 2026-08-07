Feature: lns run resource limits — vCPU count and memory size flags
  `lns run --cpus N` and `lns run -m SIZE` bound the microVM's resources,
  mirroring `docker run --cpus`/`-m`. Memory accepts Docker-style unit
  suffixes and zero-sized resources are rejected at the CLI boundary, so a
  typo never boots an unbootable VM. A definition's `spec.resources` is the
  other source, and the summary reports what the run will actually boot
  with — not just what the flags said.

  Scenario: The summary reports the definition's resources, not the flag fallback
    Given an lns.yaml declaring 3 vCPU and 6Gi of memory
    When the local run summary is composed with no resource flags
    Then the run summary shows "Resources: 3 vCPU · 6144 MiB"

  Scenario: An explicit flag outranks the definition in the summary
    Given an lns.yaml declaring 3 vCPU and 6Gi of memory
    When the local run summary is composed with "--cpus 2"
    Then the run summary shows "Resources: 2 vCPU · 6144 MiB"

  Scenario: A published sandbox's declared resources reach the summary
    Given an lns.yaml declaring 3 vCPU and 6Gi of memory
    When the published run summary is composed with no resource flags
    Then the run summary shows "Resources: 3 vCPU · 6144 MiB"

  Scenario: A definition sized in percent boots that share of this host
    Given an lns.yaml declaring 80% of this host
    When the local run summary is composed with no resource flags
    Then the run summary shows "Resources: 8 vCPU · 13107 MiB"

  Scenario: A flag still outranks a share
    Given an lns.yaml declaring 80% of this host
    When the local run summary is composed with "--cpus 2"
    Then the run summary shows "Resources: 2 vCPU · 13107 MiB"

  Scenario: A definition that declares no resources still shows the built-in default
    Given an lns.yaml declaring no resources
    When the local run summary is composed with no resource flags
    Then the run summary shows "Resources: 1 vCPU · 512 MiB"

  Scenario: Zero vCPUs is rejected before the run starts
    When the user runs `lns run --cpus 0 someimage`
    Then the command fails with a parse error naming the --cpus flag
    And no run is started

  Scenario: A memory size with a unit suffix is accepted
    Given the command is `lns run -m 2g someimage`
    When the summary is printed
    Then the summary shows `Resources: 1 vCPU · 2048 MiB`

  Scenario: The flag accepts the same unit a definition writes
    Given the command is `lns run -m 38Gi someimage`
    When the summary is printed
    Then the summary shows `Resources: 1 vCPU · 38912 MiB`

  Scenario: --memory works as an alias of --mem
    Given the command is `lns run --memory 1g someimage`
    When the summary is printed
    Then the summary shows `Resources: 1 vCPU · 1024 MiB`

  Scenario: Zero memory is rejected before the run starts
    When the user runs `lns run -m 0 someimage`
    Then the command fails with a parse error naming the --mem flag
    And no run is started

  Scenario: A garbled memory size is rejected before the run starts
    When the user runs `lns run -m 12parsecs someimage`
    Then the command fails with a parse error naming the --mem flag
    And no run is started
