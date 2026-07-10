Feature: managing cached sandboxes
  A sandbox is either cached or running. The manage verbs list, inspect,
  remove, prune, and diff cached sandboxes. Sandbox GC owns only the
  reconstructible cache (artifacts + base-image layers + composefs/content)
  and never touches a named volume.

  Scenario: ls lists cached sandboxes
    Given the service reports one cached sandbox "hermes:1.4.0"
    When the user runs sandbox command "ls"
    Then the exit code is 0
    And the output contains "hermes:1.4.0"
    And the output contains "cached"

  Scenario: there is no top-level ls shortcut for the cached list
    When I run "lns ls"
    Then the exit code is 2

  Scenario: inspect on a running sandbox shows its live state
    Given the reference "reviewer" resolves to a running sandbox
    When the user runs "lns inspect reviewer"
    Then the exit code is 0
    And the output contains "running"
    And the output contains "uptime"

  Scenario: inspect on a cached sandbox shows its definition
    Given the reference "hermes:1.4.0" resolves to a cached sandbox
    When the user runs "lns inspect hermes:1.4.0"
    Then the exit code is 0
    And the output contains "kind: Sandbox"
    And the output contains "image"

  @todo
  Scenario: rm refuses a running sandbox
    Given the reference "reviewer" resolves to a running sandbox
    When the user runs sandbox command "rm reviewer"
    Then the command fails with an exit code other than 0
    And the output contains "running"

  @todo
  Scenario: rm removes a cached sandbox and frees its now-unreferenced layers
    Given the sandbox "hermes:1.4.0" is cached and no other sandbox shares its base-image layers
    When the user runs sandbox command "rm hermes:1.4.0"
    Then the exit code is 0
    And the output contains "removed"
    And the output reports reclaimed base-image layers

  @todo
  Scenario: prune sweeps the reconstructible cache and reports reclaimed bytes
    Given two cached sandboxes and one running sandbox
    When the user runs sandbox command "prune --force"
    Then the exit code is 0
    And the output reports reclaimed bytes
    And the running sandbox and its layers are kept

  @todo
  Scenario: prune never removes a named volume
    Given a cached sandbox that names a volume "claude-home"
    When the user runs sandbox command "prune --force"
    Then the exit code is 0
    And the named volume "claude-home" still exists

  @todo
  Scenario: diff shows local edits and accreted grants against the pulled version
    Given the sandbox "hermes:1.4.0" was pulled and then locally edited
    When the user runs sandbox command "diff hermes:1.4.0"
    Then the exit code is 0
    And the output contains "policy"
    And the output shows the local changes since the pulled version
