Feature: resolving the lns run reference
  A path-shaped REF (., lns.yaml, ./dir, /abs/path) names a local sandbox
  definition; anything else is a registry coordinate. Resolution happens
  client-side, before any service round-trip, so these scenarios drive the
  real binary with no daemon.

  Scenario: a path-shaped reference without a definition points at lns init
    When I run "lns run ./missing" in the project directory
    Then the exit code is non-zero
    And the output contains "no lns.yaml in"
    And the output contains "lns init"

  Scenario: naming lns.yaml runs the file, not a registry coordinate
    Given a project definition at "lns.yaml" missing its image
    When I run "lns run lns.yaml" in the project directory
    Then the exit code is non-zero
    And the output contains "must carry an image"

  Scenario: dot resolves the current directory's definition
    Given a project definition at "lns.yaml" missing its image
    When I run "lns run ." in the project directory
    Then the exit code is non-zero
    And the output contains "must carry an image"

  Scenario: a relative directory reference resolves that directory's definition
    Given a project definition at "inner/lns.yaml" missing its image
    When I run "lns run ./inner" in the project directory
    Then the exit code is non-zero
    And the output contains "must carry an image"
