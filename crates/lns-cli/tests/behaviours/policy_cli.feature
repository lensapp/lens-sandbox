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

  # The note is not part of the grant, so annotating a rule the file already holds is an edit of that rule, not a second one to place in front of it.
  Scenario: Annotating a rule the file already holds edits it in place
    Given "lns-policy.yaml" has an allow rule for "api.example.test"
    When the developer adds an allow rule for "api.example.test" with description "issue sync"
    Then "lns-policy.yaml" still holds 1 rule
    And "lns-policy.yaml" contains the allow rule with the description
    And the output says the description was updated

  Scenario: Annotating a deny rule the file already holds edits it in place
    Given "lns-policy.yaml" has a deny rule for "evil.example"
    When the developer denies "evil.example" with description "phishing kit"
    Then "lns-policy.yaml" still holds 1 rule
    And the output says the description was updated

  Scenario: Re-adding a rule without a description leaves the note it already carries
    Given "lns-policy.yaml" has an allow rule for "api.example.test" described as "issue sync"
    When the developer adds an allow rule for "api.example.test" without passing --policy
    Then "lns-policy.yaml" still holds 1 rule
    And "lns-policy.yaml" contains the allow rule with the description
    And the output says the rule is already there

  # A scoped rule claims the destination and fails closed, denying every caller off the list rather than falling through, which is why the flag exists only for allow.
  Scenario: Scoping an allow rule to one binary records it in the policy file
    When the developer adds an allow rule for "git.example.test" scoped to "/usr/bin/git"
    Then "lns-policy.yaml" scopes the allow rule for "git.example.test" to "/usr/bin/git"

  Scenario: Repeating the flag scopes one rule to several binaries
    When the developer adds an allow rule for "git.example.test" scoped to "/usr/bin/git" and "/usr/bin/curl"
    Then "lns-policy.yaml" scopes the allow rule for "git.example.test" to "/usr/bin/git,/usr/bin/curl"

  # The scoping is what denies without asking, not where the rule landed, so the one case a developer meets first — a fresh file with nothing to sit in front of — must say so too.
  Scenario: Scoping a destination no other rule covers still says who it now denies
    Given the developer is in a directory with no "lns-policy.yaml"
    When the developer adds an allow rule for "git.example.test" scoped to "/usr/bin/git"
    Then the output says every other caller is now denied "git.example.test"

  Scenario: Adding an unscoped rule claims nothing about other callers
    Given the developer is in a directory with no "lns-policy.yaml"
    When the developer adds an allow rule for "api.example.test" without passing --policy
    Then the output does not claim any caller is denied

  Scenario: A relative binary path is refused before anything is written
    Given the developer is in a directory with no "lns-policy.yaml"
    When the developer adds an allow rule for "git.example.test" scoped to "git"
    Then the command fails with an exit code other than 0
    And the failure says a relative path can never match the kernel-resolved path
    And "./lns-policy.yaml" is not created

  Scenario: A binary path climbing through .. is refused before anything is written
    Given the developer is in a directory with no "lns-policy.yaml"
    When the developer adds an allow rule for "git.example.test" scoped to "/usr/bin/../bin/git"
    Then the command fails with an exit code other than 0
    And the failure says a .. segment can never match the kernel-resolved path
    And "./lns-policy.yaml" is not created

  # The gate stops at the first matching rule, so a narrowing rule written after an open one would never fire — the CLI puts it where it does what the developer asked and says so.
  Scenario: Narrowing a destination an open rule already allows puts the scoped rule first
    Given "lns-policy.yaml" has an allow rule for "api.example.test"
    When the developer adds an allow rule for "api.example.test" scoped to "/usr/bin/curl"
    Then "lns-policy.yaml" lists the rule scoped to "/usr/bin/curl" before the rule for "api.example.test"
    And the output says it was placed before the rule for "api.example.test"
    And the output says every other caller is now denied "api.example.test"

  Scenario: A wildcard allow covering the destination is what the scoped rule goes in front of
    Given "lns-policy.yaml" has an allow rule for "*.example.test"
    When the developer adds an allow rule for "api.example.test" scoped to "/usr/bin/curl"
    Then "lns-policy.yaml" lists the rule scoped to "/usr/bin/curl" before the rule for "*.example.test"

  Scenario: Scoping a whole suffix behind a catch-all allow puts the scoped rule first
    Given "lns-policy.yaml" has an allow rule for "*"
    When the developer adds an allow rule for "*.example.test" scoped to "/usr/bin/git"
    Then "lns-policy.yaml" lists the rule scoped to "/usr/bin/git" before the rule for "*"
    And the output says it was placed before the rule for "*"

  # The only place an open rule reaches the callers a scoped rule excludes is in front of it, which opens the destination to everyone — the same widening the CLI refuses to perform behind a deny.
  Scenario: Re-opening a destination a scoped rule claimed is refused rather than reordered
    Given "lns-policy.yaml" has an allow rule for "git.example.test" scoped to "/usr/bin/git"
    When the developer adds an allow rule for "git.example.test" without passing --policy
    Then the command fails with an exit code other than 0
    And the failure says the rule would open the destination to every caller
    And the error names "/usr/bin/git"
    And "lns-policy.yaml" still holds 1 rule

  Scenario: The refusal names the covering rule when it is a wildcard the developer did not type
    Given "lns-policy.yaml" has an allow rule for "*.example.test" scoped to "/usr/bin/git"
    When the developer adds an allow rule for "api.example.test" without passing --policy
    Then the command fails with an exit code other than 0
    And the error names "*.example.test"
    And "lns-policy.yaml" still holds 1 rule

  # A narrowing rule is not a widening: the caller it names already reaches the destination through the rule it goes in front of.
  Scenario: Narrowing a scoped rule to fewer of its binaries is placed in front of it
    Given "lns-policy.yaml" has an allow rule for "git.example.test" scoped to "/usr/bin/git" and "/usr/bin/curl"
    When the developer adds an allow rule for "git.example.test" scoped to "/usr/bin/git"
    Then "lns-policy.yaml" lists the rule scoped to "/usr/bin/git" before the rule scoped to "/usr/bin/git,/usr/bin/curl"
    And the output does not claim any scoping is spent

  Scenario: Denying a destination a scoped allow claimed puts the deny in front of it
    Given "lns-policy.yaml" has an allow rule for "git.example.test" scoped to "/usr/bin/git"
    When the developer denies "git.example.test"
    Then "lns-policy.yaml" lists the deny rule for "git.example.test" before the rule scoped to "/usr/bin/git"
    And the output says the scoped rule for "git.example.test" no longer applies

  # A narrowing of who may reach a destination is not a request to stop intercepting it, so the rule going in front carries the fronted rule's TLS termination with it.
  Scenario: A rule placed in front of a TLS-terminating rule keeps terminating TLS
    Given "lns-policy.yaml" has a TLS-terminating allow rule for "api.example.test"
    When the developer adds an allow rule for "api.example.test" scoped to "/usr/bin/curl"
    Then "lns-policy.yaml" terminates TLS on the rule scoped to "/usr/bin/curl"
    And the output says the placed rule terminates TLS too

  Scenario: A deny placed in front of a TLS-terminating rule does not take up terminating TLS
    Given "lns-policy.yaml" has a TLS-terminating allow rule for "api.example.test"
    When the developer denies "api.example.test"
    Then "lns-policy.yaml" does not terminate TLS on the deny rule for "api.example.test"

  # Inheriting the termination makes the new rule the one the file already holds, so it is an annotation of that rule — not a second copy of it that drops its note.
  Scenario: Re-adding a rule the file holds as a TLS-terminating one keeps its note
    Given "lns-policy.yaml" has a TLS-terminating allow rule for "api.example.test" described as "issue sync"
    When the developer adds an allow rule for "api.example.test" without passing --policy
    Then the output says the rule is already there
    And "lns-policy.yaml" still holds 1 rule
    And "lns-policy.yaml" describes the allow rule for "api.example.test" as "issue sync"

  # A rule carrying an http `rules` list allows only the requests it names and denies the rest, so getting in front of it hands the new rule's callers every method and path.
  Scenario: Fronting a rule that restricts which requests it allows is refused
    Given "lns-policy.yaml" has an allow rule for "api.example.test" restricted to GET requests
    When the developer adds an allow rule for "api.example.test" scoped to "/usr/bin/curl"
    Then the command fails with an exit code other than 0
    And the failure says the rule would lift the request restriction
    And "lns-policy.yaml" still holds 1 rule

  Scenario: Denying a destination whose rule restricts requests is still placed in front of it
    Given "lns-policy.yaml" has an allow rule for "api.example.test" restricted to GET requests
    When the developer denies "api.example.test"
    Then "lns-policy.yaml" lists the deny rule for "api.example.test" before the rule restricted to GET requests

  # Removing the deny would widen egress for every destination it covers, so the refusal must not send the developer there.
  Scenario: A destination an earlier deny already blocks is refused rather than reordered
    Given "lns-policy.yaml" has an allow rule for "api.example.test" and a deny rule for "evil.example"
    When the developer adds an allow rule for "evil.example" scoped to "/usr/bin/curl"
    Then the command fails with an exit code other than 0
    And the failure does not tell the developer to remove the deny
    And the error names "evil.example"
    And "lns-policy.yaml" still holds 2 rules

  Scenario: Adding a rule the file already holds changes nothing
    Given "lns-policy.yaml" has an allow rule for "api.example.test"
    When the developer adds an allow rule for "api.example.test" without passing --policy
    Then the output says the rule is already there
    And "lns-policy.yaml" still holds 1 rule

  # The file holding a grant and the gate reaching it are different things: a copy stranded behind a rule that pre-empts it is not the rule to report as already in force.
  Scenario: A grant the file holds but a deny pre-empts is refused, not reported as already there
    Given "lns-policy.yaml" has a deny rule for "*.example.test" ahead of an allow rule for "api.example.test"
    When the developer adds an allow rule for "api.example.test" without passing --policy
    Then the command fails with an exit code other than 0
    And the error names "*.example.test"
    And "lns-policy.yaml" still holds 2 rules

  Scenario: A deny the file holds but an allow pre-empts is moved in front of that allow
    Given "lns-policy.yaml" has an allow rule for "api.example.test" ahead of a deny rule for "api.example.test"
    When the developer denies "api.example.test"
    Then "lns-policy.yaml" lists the deny rule for "api.example.test" before the allow rule for "api.example.test"
    And "lns-policy.yaml" still holds 2 rules

  Scenario: Moving a stranded rule into force keeps the note it was carrying
    Given "lns-policy.yaml" has an allow rule for "api.example.test" ahead of a deny rule for "api.example.test" described as "phishing kit"
    When the developer denies "api.example.test"
    Then "lns-policy.yaml" still holds 2 rules
    And "lns-policy.yaml" describes the deny rule for "api.example.test" as "phishing kit"

  # A same-verdict rule can pre-empt by scope or by request filter as surely as a deny pre-empts by verdict, and a copy stranded behind one of those is no more in force.
  Scenario: A grant a binary-scoped rule pre-empts is refused, not reported as already there
    Given "lns-policy.yaml" has an allow rule for "git.example.test" scoped to "/usr/bin/git" ahead of an unscoped allow rule for "git.example.test"
    When the developer adds an allow rule for "git.example.test" without passing --policy
    Then the command fails with an exit code other than 0
    And the failure says the rule would open the destination to every caller
    And "lns-policy.yaml" still holds 2 rules

  Scenario: A grant a request-filtered rule pre-empts is refused, not reported as already there
    Given "lns-policy.yaml" has an allow rule for "api.example.test" restricted to GET requests ahead of an unrestricted allow rule for "api.example.test"
    When the developer adds an allow rule for "api.example.test" without passing --policy
    Then the command fails with an exit code other than 0
    And the failure says the rule would lift the request restriction
    And "lns-policy.yaml" still holds 2 rules

  # A deny behind a deny is the one shadowed rule that is not a mistake: the destination is already blocked, which is what the developer asked for.
  Scenario: Denying a destination a broader deny already blocks succeeds and changes nothing
    Given "lns-policy.yaml" has a deny rule for "*.example.test"
    When the developer denies "api.example.test"
    Then the command succeeds
    And the output says the deny adds nothing
    And "lns-policy.yaml" still holds 1 rule

  # `lns policy remove` deletes every rule carrying the pattern, scoped or not, so the scoping has to be visible before removing anything.
  Scenario: Listing shows which binaries a rule is scoped to
    Given "lns-policy.yaml" has an allow rule for "git.example.test" scoped to "/usr/bin/git"
    When the developer lists rules
    Then the output shows the rule scoped to "/usr/bin/git"

  Scenario: Listing as JSON tells a script which rules are binary-scoped
    Given "lns-policy.yaml" has an allow rule for "api.example.test" and a scoped allow rule for "git.example.test"
    When the developer lists rules as JSON
    Then JSON row 0 has a null "binaries"
    And JSON row 1 has "binaries" set to the list "/usr/bin/git"

  Scenario: Listing rules shows what is currently in the policy file
    Given "lns-policy.yaml" has an allow rule for "api.linear.app" and a deny rule for "evil.example"
    When the developer lists rules
    Then the output shows both rules with their verdicts

  Scenario: Listing rules from a policy file naming the removed allowedRoutes key fails loudly
    Given "lns-policy.yaml" uses the removed allowedRoutes key for "api.linear.app"
    When the developer lists rules
    Then the command fails with an exit code other than 0
    And the error names "allowedRoutes"

  Scenario: Removing a rule by pattern deletes it from the policy file
    Given "lns-policy.yaml" has an allow rule for "api.linear.app"
    When the developer removes the rule matching "api.linear.app"
    Then "lns-policy.yaml" no longer contains a rule for "api.linear.app"

  # Removal goes by pattern alone, so a scoped rule can go with the one the developer meant; the report has to name the count or a silent extra deletion reads as a single-rule removal.
  Scenario: Removing a destination several rules carry says how many went
    Given "lns-policy.yaml" has an allow rule for "git.example.test" scoped to "/usr/bin/git" ahead of an unscoped allow rule for "git.example.test"
    When the developer removes the rule matching "git.example.test"
    Then the output says 2 rules were removed for "git.example.test"

  Scenario: Removing a destination one rule carries reports it in the singular
    Given "lns-policy.yaml" has an allow rule for "api.linear.app"
    When the developer removes the rule matching "api.linear.app"
    Then the output says 1 rule was removed for "api.linear.app"

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
