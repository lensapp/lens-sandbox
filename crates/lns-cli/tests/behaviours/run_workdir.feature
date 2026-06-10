Feature: lns run -w sets the workload working directory
  `lns run -w DIR` starts the workload in DIR inside the sandbox,
  mirroring `docker run -w`. The path is a guest path, so it must be
  absolute — a relative path is rejected at the CLI boundary before any
  service round-trip.

  Scenario: A relative workdir is rejected before the run starts
    When the user runs `lns run -w app someimage`
    Then the command fails with a parse error naming the --workdir flag
    And no run is started

  Scenario: The summary shows the requested working directory
    Given the command is `lns run -w /app someimage`
    When the summary is printed
    Then the summary shows `Workdir:   /app`
