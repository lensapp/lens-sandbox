Feature: lns run resource limits — vCPU count and memory size flags
  `lns run --cpus N` and `lns run -m SIZE` bound the microVM's resources,
  mirroring `docker run --cpus`/`-m`. Memory accepts Docker-style unit
  suffixes and zero-sized resources are rejected at the CLI boundary, so a
  typo never boots an unbootable VM.

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
