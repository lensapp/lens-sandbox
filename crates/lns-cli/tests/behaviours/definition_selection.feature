Feature: selecting the sandbox definition file
  `./lns.yaml` is the sandbox — one directory is one sandbox. A path-shaped
  reference naming a `.yaml` file, or the explicit `-f/--file` selector, is
  the override that operates on a different definition file: the file's
  directory is the project, so it roots the relative binds and filesets,
  compose-style, and holds the decisions the run resolves. Nothing names the
  decisions file: it is always the one beside the definition.

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

  Scenario: validate --file validates the named definition
    Given a sandbox definition file "lns.dev.yaml" in the current directory
    When the user runs artifact command "validate --file lns.dev.yaml"
    Then the exit code is 0
    And the output contains "valid"
    And the service received no request

  Scenario: push --file publishes the named definition
    Given a sandbox definition file "lns.dev.yaml" in the current directory
    And the registry accepts the push
    When the user runs "lns push --file lns.dev.yaml ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the pushed artifact carries the definition from "lns.dev.yaml"

  Scenario: inspect renders a named yaml file's definition offline
    Given a sandbox definition file "lns.dev.yaml" in the current directory
    When the user runs artifact command "inspect ./lns.dev.yaml"
    Then the exit code is 0
    And the output contains "image"
    And the service received no request

  Scenario: a named definition file that does not exist errors with its path
    Given the current directory has no lns.yaml
    When the user runs "lns run ./lns.dev.yaml"
    Then the command fails with an exit code other than 0
    And the output contains "no sandbox definition at"
    And the output contains "lns.dev.yaml"

  Scenario: a definition in another directory is governed by that directory's decisions
    Given a sandbox definition file "/other/lns.dev.yaml" declaring a relative bind and fileset
    When the user runs "lns run /other/lns.dev.yaml"
    Then the exit code is 0
    And the run reads its decisions from "/other/lns-local-mixin.yaml"

  Scenario: no flag can point the run at another decisions file
    Given a sandbox definition file "/other/lns.dev.yaml" declaring a relative bind and fileset
    When the user runs "lns run --policy team.yaml /other/lns.dev.yaml"
    Then the exit code is 2
    And the output contains "unexpected argument"
