@microvm
Feature: a booted run records its events to the durable audit trail
  A real run writes its per-run audit log under the durable data directory
  (not the ephemeral cache), and `lns audit` surfaces the events from a real
  guest. The read side is pinned separately at Layer 1 against seeded logs;
  this proves the live relay → audit-log → `lns audit` path end to end.

  Scenario: an injected env var is recorded and shown by lns audit
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'true'" with env "E2E_AUDIT_VAR=recorded-123"
    Then the exit code is 0
    And "lns audit" for that run reports an "env" event naming "E2E_AUDIT_VAR"
