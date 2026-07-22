Feature: selecting the sandbox definition file
  `./lns.yaml` is the sandbox — one directory is one sandbox. A path-shaped
  reference naming a `.yaml` file, or the explicit `-f/--file` selector, is
  the override that operates on a different definition file: the file's
  directory roots its relative binds and filesets, compose-style, while the
  policy still comes from where you run.

  Scenario: a path-shaped reference naming a yaml file runs that file's definition
    Given a sandbox definition file "lns.dev.yaml" in the current directory
    When the user runs "lns run ./lns.dev.yaml"
    Then the exit code is 0
    And the service request carries the definition from "lns.dev.yaml"

  Scenario: a yaml file reference in another directory roots relative sources at the file's directory
    Given a sandbox definition file "/other/lns.dev.yaml" declaring a relative bind and fileset
    When the user runs "lns run /other/lns.dev.yaml"
    Then the exit code is 0
    And the service request roots the bind and fileset at "/other"

  Scenario: --file selects the definition for a reference-less run
    Given a sandbox definition file "lns.dev.yaml" in the current directory
    When the user runs "lns run --file lns.dev.yaml"
    Then the exit code is 0
    And the service request carries the definition from "lns.dev.yaml"

  Scenario: --file combined with a registry reference is refused
    Given a sandbox definition file "lns.dev.yaml" in the current directory
    When the user runs "lns run --file lns.dev.yaml ghcr.io/team/hermes:1.4.0"
    Then the exit code is 2
    And the output contains "cannot be used with"

  @todo
  Scenario: validate --file validates the named definition
    Given a sandbox definition file "lns.dev.yaml" in the current directory
    When the user runs sandbox command "validate --file lns.dev.yaml"
    Then the exit code is 0
    And the output contains "valid"
    And the service received no request

  @todo
  Scenario: push --file publishes the named definition
    Given a sandbox definition file "lns.dev.yaml" in the current directory
    When the user runs "lns push --file lns.dev.yaml ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the pushed artifact carries the definition from "lns.dev.yaml"

  @todo
  Scenario: inspect renders a named yaml file's definition offline
    Given a sandbox definition file "lns.dev.yaml" in the current directory
    When the user runs sandbox command "inspect ./lns.dev.yaml"
    Then the exit code is 0
    And the output contains "image"
    And the service received no request

  Scenario: a named definition file that does not exist errors with its path
    Given the current directory has no lns.yaml
    When the user runs "lns run ./lns.dev.yaml"
    Then the command fails with an exit code other than 0
    And the output contains "no sandbox definition at"
    And the output contains "lns.dev.yaml"
