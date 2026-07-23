Feature: pulling a connector from a registry
  `lns connector pull <ref>` fetches a connector artifact through the
  background service and folds it into the local catalog, so it becomes
  available alongside the bundled and user-declared connectors. The
  service refuses to silently replace an already-connected connector
  whose definition changed; the CLI surfaces that as a disclosed diff
  the user re-runs with `--yes` to accept.

  These scenarios use a synthetic connector reference, so nothing here
  pins a shipped service.

  Scenario: Pulling a connector reports what landed
    When the user runs connector command "pull registry.lns.run/connectors/some-provider:0.1.0"
    Then the exit code is 0
    And the output contains "Pulled connector"
