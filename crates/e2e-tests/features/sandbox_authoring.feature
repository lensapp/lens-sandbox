Feature: authoring a sandbox definition offline
  The author verbs (init, validate, offline inspect) work on ./lns.yaml in the
  working directory — no daemon, no network. These scenarios drive the
  real binary against a real project directory: init scaffolds the
  definition, validate judges it offline, and a target-less inspect renders the effective
  definition. `lns init` is the top-level shortcut for `lns artifact init`.

  Scenario: init scaffolds a default lns.yaml
    When I run "lns init" in the project directory
    Then the exit code is 0
    And the output contains "created lns.yaml"
    And the project file "lns.yaml" contains "kind: sandbox"
    And the project file "lns.yaml" contains "workdir: /workspace"
    And the project file "lns.yaml" contains "volumes:"
    And the project file "lns.yaml" contains "tools: []"

  Scenario: validate refuses a bare tool name through the real binary
    Given a project definition declaring tool "node"
    When I run "lns artifact validate" in the project directory
    Then the exit code is non-zero
    And the output contains "node@latest"

  Scenario: validate refuses an engine backend prefix through the real binary
    Given a project definition declaring tool "npm:some-tool@3"
    When I run "lns artifact validate" in the project directory
    Then the exit code is non-zero
    And the output contains "engine backend prefix"

  Scenario: inspect lists the declared tools offline
    Given a project definition declaring tool "node@22"
    When I run "lns artifact inspect" in the project directory
    Then the exit code is 0
    And the output contains "tool: node@22"

  Scenario: init refuses to overwrite an existing lns.yaml
    When I run "lns init" in the project directory
    And I run "lns init" in the project directory
    Then the exit code is non-zero
    And the output contains "already exists"

  Scenario: validate passes the scaffolded definition
    When I run "lns artifact init" in the project directory
    And I run "lns artifact validate" in the project directory
    Then the exit code is 0
    And the output contains "lns.yaml is valid."

  Scenario: validate without a definition points at lns init
    When I run "lns artifact validate" in the project directory
    Then the exit code is non-zero
    And the output contains "run `lns init` to scaffold one"

  Scenario: inspect with no target renders the effective definition
    When I run "lns init" in the project directory
    And I run "lns artifact inspect" in the project directory
    Then the exit code is 0
    And the output contains "Sandbox: sandbox"
    And the output contains "docker.io/library/alpine:3.20"
    And the output contains "route(s)"
