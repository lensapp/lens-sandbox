Feature: authoring a sandbox definition offline
  The author verbs (init, validate, show) work on ./lns.yaml in the
  working directory — no daemon, no network. These scenarios drive the
  real binary against a real project directory: init scaffolds the
  definition, validate judges it offline, and show renders the effective
  definition. `lns init` is the top-level shortcut for `lns sandbox init`.

  Scenario: init scaffolds a default lns.yaml
    When I run "lns init" in the project directory
    Then the exit code is 0
    And the output contains "created lns.yaml"
    And the project file "lns.yaml" contains "kind: Sandbox"
    And the project file "lns.yaml" contains "workdir: /opt/data"
    And the project file "lns.yaml" contains "volumes:"
    And the project file "skills/SKILL.md" contains "name: hello-sandbox"

  Scenario: init refuses to overwrite an existing lns.yaml
    When I run "lns init" in the project directory
    And I run "lns init" in the project directory
    Then the exit code is non-zero
    And the output contains "already exists"

  Scenario: validate passes the scaffolded definition
    When I run "lns sandbox init" in the project directory
    And I run "lns sandbox validate" in the project directory
    Then the exit code is 0
    And the output contains "lns.yaml is valid."

  Scenario: validate without a definition points at lns init
    When I run "lns sandbox validate" in the project directory
    Then the exit code is non-zero
    And the output contains "run `lns init` to scaffold one"

  Scenario: show renders the effective definition
    When I run "lns init" in the project directory
    And I run "lns sandbox show" in the project directory
    Then the exit code is 0
    And the output contains "Sandbox: hermes"
    And the output contains "docker.io/nousresearch/hermes-agent:latest"
    And the output contains "defaultVerdict="
