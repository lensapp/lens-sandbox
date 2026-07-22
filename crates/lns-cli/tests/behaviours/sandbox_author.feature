Feature: authoring a sandbox
  A sandbox is authored on disk as `./lns.yaml` (kind: Sandbox). The author
  verbs scaffold it and validate it offline; `inspect` with no target (or a
  path-shaped one) renders its effective definition, also offline.

  Scenario: init scaffolds a default sandbox definition with every spec field
    Given the current directory has no lns.yaml
    When the user runs sandbox command "init"
    Then the exit code is 0
    And a file "lns.yaml" is created
    And the file "lns.yaml" contains "kind: Sandbox"
    And the file "lns.yaml" contains "apiVersion: lns.run/v1"
    And the file "lns.yaml" contains "workdir: /workspace"
    And the file "lns.yaml" contains "volumes:"
    And the file "lns.yaml" contains "env:"
    And the file "lns.yaml" contains "resources:"
    And the file "lns.yaml" contains "defaultVerdict: ask"
    And the file "lns.yaml" contains "allowedRoutes:"
    And the file "lns.yaml" contains "connectors:"
    And the file "lns.yaml" contains "credentials:"
    And the file "lns.yaml" contains "filesets:"
    And the file "lns.yaml" contains "ports:"

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

  Scenario: validate refuses an unknown nested definition field
    Given an lns.yaml with a misspelled volume readOnly field
    When the user runs sandbox command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "unknown field"
    And the service received no request

  Scenario: inspect with no target renders the effective definition offline
    Given a valid lns.yaml in the current directory
    When the user runs sandbox command "inspect"
    Then the exit code is 0
    And the output contains "image"
    And the output contains "policy"
    And the service received no request

  Scenario: inspect discloses every declared credential slot
    Given an lns.yaml declaring the "some-provider" credential as "SOME_TOKEN"
    When the user runs sandbox command "inspect"
    Then the exit code is 0
    And the output contains "credential: some-provider -> SOME_TOKEN"
    And the service received no request

  Scenario: inspect identifies a required credential slot
    Given an lns.yaml declaring the required "some-provider" credential as "SOME_TOKEN"
    When the user runs sandbox command "inspect"
    Then the exit code is 0
    And the output contains "credential: some-provider -> SOME_TOKEN (required)"
    And the service received no request

  Scenario: inspect of a path-shaped target renders the definition offline
    Given a valid lns.yaml in the current directory
    When the user runs sandbox command "inspect ."
    Then the exit code is 0
    And the output contains "image"
    And the service received no request

  Scenario: there is no standalone show command
    When I run "lns sandbox show"
    Then the exit code is 2
    And the output contains "unrecognized subcommand"

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

  Scenario: validate refuses a fileset mounted into the sandbox runtime namespace
    Given an lns.yaml declaring fileset "./skills" mounted at "/.lens/bin"
    When the user runs sandbox command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "/.lens runtime namespace"

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

  Scenario: validate and inspect understand declarative workdir and mounts
    Given an lns.yaml declaring workdir and declarative mounts
    When the user runs sandbox command "validate"
    Then the exit code is 0
    When the user runs sandbox command "inspect"
    Then the exit code is 0
    And the output contains "/workspace"
    And the output contains "bind ."
    And the output contains "volume some-cache"
