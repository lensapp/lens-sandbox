Feature: managing cached sandboxes
  A sandbox is either cached or running. The manage verbs list, inspect,
  remove, prune, and diff cached sandboxes. Sandbox GC owns only the
  reconstructible cache (artifacts + base-image layers + composefs/content)
  and never touches a named volume.

  Scenario: ls says what each cached artifact is, how big, and who holds it
    Given the service reports one cached sandbox "hermes:1.4.0"
    When the user runs artifact command "ls"
    Then the exit code is 0
    And the output contains "hermes:1.4.0"
    And the output contains "KIND"
    And the output contains "DIGEST"
    And the output contains "SIZE"
    And the output contains "HOLDER"
    And the output contains "sandbox"
    And the output contains "14.0 MiB"

  Scenario: ls names the run holding an artifact rather than leaving the column blank
    Given the service reports one cached sandbox "hermes:1.4.0" held by run 3
    When the user runs artifact command "ls"
    Then the exit code is 0
    And the output contains "000000030000"

  Scenario: ls --kind lists only the artifacts of that kind
    Given the service reports a cached sandbox "hermes:1.4.0" and a cached image "alpine:3.20"
    When the user runs artifact command "ls --kind image"
    Then the exit code is 0
    And the output contains "alpine:3.20"
    And the output does not contain "hermes:1.4.0"

  Scenario: ls --kind rejects a kind the cache cannot hold
    When I run "lns artifact ls --kind sorcery"
    Then the exit code is 2
    And the output contains "invalid value"

  Scenario: there is no top-level ls shortcut for the cached list
    When I run "lns ls"
    Then the exit code is 2

  Scenario: inspect on a running sandbox shows its live state
    Given the reference "reviewer" resolves to a running sandbox
    When the user runs "lns inspect reviewer"
    Then the exit code is 0
    And the output contains "running"
    And the output contains "UPTIME"

  Scenario: inspect on a cached sandbox shows its definition
    Given the reference "hermes:1.4.0" resolves to a cached sandbox
    When the user runs "lns inspect hermes:1.4.0"
    Then the exit code is 0
    And the output contains "kind: sandbox"
    And the output contains "image"

  Scenario: the service refuses to remove a running sandbox, and rm says how to force it
    Given the service refuses to remove the running sandbox "reviewer"
    When the user runs sandbox command "rm reviewer"
    Then the command fails with an exit code other than 0
    And the output contains "running"
    And the output contains "-f"

  Scenario: rm -f asks the service to stop the sandbox first
    Given the reference "reviewer" resolves to a running sandbox
    When the user runs sandbox command "rm -f reviewer"
    Then the exit code is 0
    And the service received a forced RemoveRun for "reviewer"

  Scenario: rm removes a cached sandbox and frees its now-unreferenced layers
    Given the sandbox "hermes:1.4.0" is cached and no other sandbox shares its base-image layers
    When the user runs artifact command "rm hermes:1.4.0"
    Then the exit code is 0
    And the output contains "removed"
    And the output reports reclaimed base-image layers

  Scenario: prune sweeps the reconstructible cache and reports reclaimed bytes
    Given two cached sandboxes and one running sandbox
    When the user runs artifact command "prune --force"
    Then the exit code is 0
    And the output reports reclaimed bytes
    And the running sandbox and its layers are kept
    And the service received a PruneImages request

  Scenario: prune asks before it sweeps, and proceeds on yes
    Given two cached sandboxes and one running sandbox
    And the user will answer "y" to the sandbox prompt
    When the user runs artifact command "prune"
    Then the exit code is 0
    And the output contains "Continue? [y/N]"
    And the output contains "provisioned tool cache"
    And the service received a PruneImages request

  Scenario: with no terminal to ask at, prune refuses rather than assuming
    Given two cached sandboxes and one running sandbox
    And sandbox input is non-interactive
    When the user runs artifact command "prune"
    Then the command fails with an exit code other than 0
    And the output contains "--force"
    And the service received no request

  Scenario: declining the prune prompt touches nothing
    Given two cached sandboxes and one running sandbox
    And the user will answer "n" to the sandbox prompt
    When the user runs artifact command "prune"
    Then the exit code is 0
    And the output contains "Aborted."
    And the service received no PruneImages request

  Scenario: prune never removes a named volume
    Given a cached sandbox that names a volume "claude-home"
    When the user runs artifact command "prune --force"
    Then the exit code is 0
    And the named volume "claude-home" still exists

  @todo
  Scenario: diff shows local edits and accreted grants against the pulled version
    Given the sandbox "hermes:1.4.0" was pulled and then locally edited
    When the user runs sandbox command "diff hermes:1.4.0"
    Then the exit code is 0
    And the output contains "egress"
    And the output shows the local changes since the pulled version

  Scenario: ls shows only the running sandboxes
    Given the service reports one running sandbox and one that stopped
    When the user runs sandbox command "ls"
    Then the exit code is 0
    And the output contains "reviewer"
    And the output does not contain "scribe"

  Scenario: ls -a includes the sandboxes that stopped
    Given the service reports one running sandbox and one that stopped
    When the user runs sandbox command "ls -a"
    Then the exit code is 0
    And the output contains "reviewer"
    And the output contains "scribe"
    And the output contains "STATE"
    And the output contains "stopped (0)"

  Scenario: a stopped sandbox is listed without sampling a guest that is not there
    Given the service reports one running sandbox and one that stopped
    When the user runs sandbox command "ls -a"
    Then the exit code is 0
    And the service asked for stats exactly once

  Scenario: prune asks before it sweeps the stopped sandboxes
    Given the service will sweep the stopped sandboxes "scribe" and "hermes"
    And the user will answer "y" to the sandbox prompt
    When the user runs sandbox command "prune"
    Then the exit code is 0
    And the output contains "Continue? [y/N]"
    And the output contains "removed sandbox hermes"
    And the output contains "removed sandbox scribe"

  Scenario: declining the prune prompt sweeps nothing
    Given the service will sweep the stopped sandboxes "scribe" and "hermes"
    And the user will answer "n" to the sandbox prompt
    When the user runs sandbox command "prune"
    Then the exit code is 0
    And the output contains "Aborted."
    And the service received no PruneRuns request

  Scenario: with no terminal to ask at, prune refuses rather than assuming
    Given the service will sweep the stopped sandboxes "scribe" and "hermes"
    And sandbox input is non-interactive
    When the user runs sandbox command "prune"
    Then the command fails with an exit code other than 0
    And the output contains "--force"
    And the service received no request

  Scenario: pruning with nothing stopped says so
    Given the service reports no stopped sandboxes to sweep
    When the user runs sandbox command "prune --force"
    Then the exit code is 0
    And the output contains "No stopped sandboxes."
