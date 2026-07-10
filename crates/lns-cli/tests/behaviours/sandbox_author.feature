Feature: authoring a sandbox
  A sandbox is authored on disk as `./lns.yaml` (kind: Sandbox). The author
  verbs scaffold it, validate it offline, and render its effective definition.

  Scenario: init scaffolds a default sandbox definition
    Given the current directory has no lns.yaml
    When the user runs sandbox command "init"
    Then the exit code is 0
    And a file "lns.yaml" is created
    And the file "lns.yaml" contains "kind: Sandbox"
    And the file "lns.yaml" contains "apiVersion: lns.run/v1"

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

  Scenario: validate runs the schema, cross-field, and secret checks offline
    Given a valid lns.yaml in the current directory
    When the user runs sandbox command "validate"
    Then the exit code is 0
    And the output contains "valid"
    And the service received no request

  Scenario: validate refuses a definition carrying a real secret
    Given an lns.yaml whose env carries a value shaped like a real secret
    When the user runs sandbox command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "secret"
    And the output contains "placeholder"

  Scenario: show renders the effective definition offline
    Given a valid lns.yaml in the current directory
    When the user runs sandbox command "show"
    Then the exit code is 0
    And the output contains "image"
    And the output contains "policy"
    And the service received no request
