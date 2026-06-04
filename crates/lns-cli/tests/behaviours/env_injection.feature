Feature: lns run -e rejects malformed env arguments at the CLI boundary
  `lns run -e KEY=VALUE` injects non-secret configuration into the
  workload, mirroring `docker run -e`. Malformed forms are rejected by
  the CLI before any service round-trip, so a typo never silently
  starts a run with a missing or mis-shaped variable. Bare `-e KEY`
  (host passthrough) is deliberately unsupported — it would be a silent
  host-to-workload leak channel.

  Scenario: An empty key is rejected before the run starts
    When the user runs `lns run -e =oops someimage`
    Then the command fails with a parse error naming the bad -e argument
    And no run is started

  Scenario: A bare -e KEY with no '=' is rejected (no host passthrough)
    When the user runs `lns run -e HOME someimage`
    Then the command fails with a parse error requiring KEY=VALUE form
    And no run is started
