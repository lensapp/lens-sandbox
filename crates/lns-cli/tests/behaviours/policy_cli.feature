Feature: shaping network rules from the CLI
  The approval window is the interactive way to decide network rules,
  but it needs a running workload and a human clicking. The CLI is the
  non-interactive counterpart: it writes rules straight into the policy
  file in use — the same `lns-policy.yaml` that `lns run` loads and that
  `--policy` selects — so a developer can pre-seed rules before a run or
  adjust them while one is in flight. The CLI writes the file directly;
  the running sandbox's file watcher hot-swaps the change, so these
  commands work whether or not the service or any sandbox is running.

  Scenario: Adding an allow rule writes it to the policy file in use
    Given no sandbox is running
    When the developer adds an allow rule for "api.linear.app" to "lns-policy.yaml"
    Then "lns-policy.yaml" contains an allow rule for "api.linear.app"

  # The CLI's job is to write the rule into the file the running sandbox loaded;
  # the running sandbox then hot-swaps that external write via its PolicyWatcher.
  # That reload is covered by lns-service approval_flow.feature
  # ("A manual edit to the policy file hot-swaps the running policy"), so here we
  # pin only the CLI half: a no-flag add targets the in-use cwd policy file.
  Scenario: Adding a rule while a sandbox is running targets the policy file it loaded
    Given a sandbox is running with "lns-policy.yaml" loaded in the current directory
    When the developer adds an allow rule for "api.linear.app" without passing --policy
    Then "lns-policy.yaml" in the current directory contains an allow rule for "api.linear.app"

  Scenario: --policy defaults to lns-policy.yaml in the current directory
    Given the developer is in a directory with no "lns-policy.yaml"
    When the developer adds an allow rule for "api.linear.app" without passing --policy
    Then "lns-policy.yaml" is created in the current directory
    And it contains an allow rule for "api.linear.app"
    And its defaultVerdict is "ask"

  Scenario: --policy can point at any path
    When the developer adds an allow rule for "api.linear.app" with --policy "/tmp/team-policy.yaml"
    Then "/tmp/team-policy.yaml" contains the allow rule
    And "./lns-policy.yaml" is not created

  Scenario: Adding a rule with a description records it in the policy file
    When the developer adds an allow rule for "api.linear.app" with description "Linear API for issue sync"
    Then "lns-policy.yaml" contains the allow rule with the description

  Scenario: Listing rules shows what is currently in the policy file
    Given "lns-policy.yaml" has an allow rule for "api.linear.app" and a deny rule for "evil.example"
    When the developer lists rules
    Then the output shows both rules with their verdicts

  Scenario: Removing a rule by pattern deletes it from the policy file
    Given "lns-policy.yaml" has an allow rule for "api.linear.app"
    When the developer removes the rule matching "api.linear.app"
    Then "lns-policy.yaml" no longer contains a rule for "api.linear.app"

  Scenario: Removing a rule that does not exist surfaces a clear error
    Given "lns-policy.yaml" has no rule for "ghost.example"
    When the developer tries to remove a rule for "ghost.example"
    Then the command fails with an exit code other than 0
    And the policy file is unchanged

  Scenario: Listing rules as JSON gives a script the verdict and pattern per rule
    Given "lns-policy.yaml" has an allow rule for "api.linear.app" and a deny rule for "evil.example"
    When the developer lists rules as JSON
    Then the output is a JSON array of 2 rows
    And JSON row 0 has "verdict" set to "allow"
    And JSON row 0 has "pattern" set to "api.linear.app"
    And JSON row 1 has "verdict" set to "deny"
    And JSON row 1 has "pattern" set to "evil.example"

  Scenario: An empty policy lists as an empty JSON array, not prose
    Given the developer is in a directory with no "lns-policy.yaml"
    When the developer lists rules as JSON
    Then the output is an empty JSON array
