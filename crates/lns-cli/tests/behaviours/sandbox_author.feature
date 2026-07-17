Feature: authoring a sandbox
  A sandbox is authored on disk as `./lns.yaml` (kind: Sandbox). The author
  verbs scaffold it, validate it offline, and render its effective definition.

  Scenario: init scaffolds an agent example with every spec field
    Given the current directory has no lns.yaml
    When the user runs sandbox command "init"
    Then the exit code is 0
    And a file "lns.yaml" is created
    And the file "lns.yaml" contains "kind: Sandbox"
    And the file "lns.yaml" contains "apiVersion: lns.run/v1"
    And the file "lns.yaml" contains "image: docker.io/nousresearch/hermes-agent:latest"
    And the file "lns.yaml" contains "command: gateway run"
    And the file "lns.yaml" contains "workdir: /opt/data"
    And the file "lns.yaml" contains "volumes:"
    And the file "lns.yaml" contains "env:"
    And the file "lns.yaml" contains "resources:"
    And the file "lns.yaml" contains "defaultVerdict: ask"
    And the file "lns.yaml" contains "allowedRoutes:"
    And the file "lns.yaml" contains "integrations:"
    And the file "lns.yaml" contains "credentials:"
    And the file "lns.yaml" contains "filesets:"
    And the file "lns.yaml" contains "ports:"
    And a file "skills/SKILL.md" is created

  Scenario: init keeps an existing skills directory untouched
    Given the current directory has no lns.yaml
    And the project directory "./skills" contains "SKILL.md"
    When the user runs sandbox command "init"
    Then the exit code is 0
    And the file "skills/SKILL.md" contains "fixture contents"

  Scenario: the scaffolded definition is valid as written
    Given the current directory has no lns.yaml
    When the user runs sandbox command "init"
    And the user runs sandbox command "validate"
    Then the exit code is 0
    And the output contains "valid"

  Scenario: init refuses to clobber an existing definition
    Given the current directory already has an lns.yaml
    When the user runs sandbox command "init"
    Then the command fails with an exit code other than 0
    And the output contains "already exists"
    And the existing lns.yaml is left unchanged

  Scenario: init takes no flags
    When I run "lns init --image alpine"
    Then the exit code is 2
    And the output contains "unexpected argument"

  Scenario: validate runs the schema and cross-field checks offline
    Given a valid lns.yaml in the current directory
    When the user runs sandbox command "validate"
    Then the exit code is 0
    And the output contains "valid"
    And the service received no request

  Scenario: show renders the effective definition offline
    Given a valid lns.yaml in the current directory
    When the user runs sandbox command "show"
    Then the exit code is 0
    And the output contains "image"
    And the output contains "policy"
    And the service received no request

  Scenario: validate accepts a path fileset whose directory exists
    Given an lns.yaml declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains "prompts.md"
    When the user runs sandbox command "validate"
    Then the exit code is 0

  Scenario: validate refuses a path fileset whose directory is missing
    Given an lns.yaml declaring fileset "./skills" mounted at "/root/.agent/skills"
    When the user runs sandbox command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "./skills"

  Scenario: validate refuses a fileset entry with both path and ref, or neither
    Given an lns.yaml declaring a fileset entry with both path and ref
    When the user runs sandbox command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "either path or ref"

  Scenario: validate refuses a relative fileset mountPath
    Given an lns.yaml declaring fileset "./skills" mounted at "skills"
    When the user runs sandbox command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "absolute"

  Scenario: validate refuses a duplicate fileset mountPath or one colliding with a volume target
    Given an lns.yaml declaring two filesets mounted at "/root/.agent/skills"
    When the user runs sandbox command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "duplicate"

  Scenario: validate refuses a secret-shaped file inside a path fileset
    Given an lns.yaml declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains ".env"
    When the user runs sandbox command "validate"
    Then the command fails with an exit code other than 0
    And the output contains ".env"

  Scenario: validate and show understand declarative workdir and mounts
    Given an lns.yaml declaring workdir and declarative mounts
    When the user runs sandbox command "validate"
    Then the exit code is 0
    When the user runs sandbox command "show"
    Then the exit code is 0
    And the output contains "/workspace"
    And the output contains "bind ."
    And the output contains "volume some-cache"
