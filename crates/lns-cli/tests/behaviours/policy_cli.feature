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

  # A catch-all deny is the file's backstop, not a decision about any one destination
  # — and it is what closes a directory now that no default can. Refusing to widen it
  # would leave a closed policy editable only by hand, since it raises no cards either.
  Scenario: Allowing a destination in a closed policy puts the allow ahead of the catch-all
    Given "lns-policy.yaml" has a deny rule for "*"
    When the developer adds an allow rule for "api.example.test" without passing --policy
    Then "lns-policy.yaml" lists the allow rule for "api.example.test" before the rule for "*"
    And the output says it was placed before the rule for "*"

  # A deny the author aimed at a destination is a decision, so an allow behind it is
  # still refused rather than quietly reordered.
  Scenario: Allowing a destination a narrower deny names is still refused
    Given "lns-policy.yaml" has a deny rule for "*.example.test"
    When the developer adds an allow rule for "api.example.test" without passing --policy
    Then the command fails with an exit code other than 0
    And the failure says the deny already blocks it

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

  # `egress.tcp` is a pre-filter with no default of its own: a destination it does not
  # name falls through to the HTTP table, so a raw splice only ever happens where the
  # developer declared it. Every raw rule is port-scoped for the same reason — the
  # traffic is passed through unread, so "any port on this host" is not a grant we offer.
  Scenario: Allowing a raw TCP destination writes it to the raw table
    When the developer allows the raw destination "db.internal:5432"
    Then "lns-policy.yaml" contains a raw allow rule for "db.internal:5432"

  Scenario: Denying a raw TCP destination writes it to the raw table
    When the developer denies the raw destination "db.internal:5432"
    Then "lns-policy.yaml" contains a raw deny rule for "db.internal:5432"

  # A portless raw rule fails the whole policy closed inside the guest, so it is
  # refused here rather than at the next run.
  Scenario: A raw destination with no port is refused before anything is written
    Given the developer is in a directory with no "lns-policy.yaml"
    When the developer allows the raw destination "db.internal"
    Then the command fails with an exit code other than 0
    And the error explains that a raw destination needs a port
    And "./lns-policy.yaml" is not created

  # The raw table is first-match-wins too, so the same placement the HTTP verbs do
  # applies here: a rule that would never be reached is not quietly written.
  Scenario: A raw allow behind a raw deny for the same destination is refused
    Given "lns-policy.yaml" has a raw deny rule for "10.0.0.0/8:5432"
    When the developer allows the raw destination "10.0.0.5:5432"
    Then the command fails with an exit code other than 0
    And the error explains the raw rule would never fire
    And the policy file is unchanged

  # A scoped raw rule claims the destination and fails closed for every caller off
  # its list, so placing an unscoped rule in front widens the file's own grant —
  # the developer's call to make, not the tool's.
  Scenario: An unscoped raw allow in front of a binaries-scoped raw rule is refused
    Given "lns-policy.yaml" has a raw allow rule for "db.internal:5432" scoped to "/usr/bin/psql"
    When the developer allows the raw destination "db.internal:5432"
    Then the command fails with an exit code other than 0
    And the error explains it would open the raw destination to every caller
    And the policy file is unchanged

  Scenario: An unscoped raw allow behind a scoped raw range names the range to narrow
    Given "lns-policy.yaml" has a raw allow rule for "10.0.0.0/24:5432" scoped to "/usr/bin/psql"
    When the developer allows the raw destination "10.0.0.5:5432"
    Then the command fails with an exit code other than 0
    And the error tells the developer to narrow the raw rule for "10.0.0.0/24:5432"

  # A deny needs no scoping to reach every caller, so it lands in front of the scoped
  # rule — which then decides nobody, and the file would otherwise still read as if
  # that one binary could open the destination.
  Scenario: A raw deny placed in front of a scoped raw allow says the scoping stopped applying
    Given "lns-policy.yaml" has a raw allow rule for "db.internal:5432" scoped to "/usr/bin/psql"
    When the developer denies the raw destination "db.internal:5432"
    Then "lns-policy.yaml" contains a raw deny rule for "db.internal:5432"
    And the output says the scoping of the rule behind it no longer applies

  Scenario: A raw deny a broader raw deny already covers adds nothing
    Given "lns-policy.yaml" has a raw deny rule for "10.0.0.0/8:5432"
    When the developer denies the raw destination "10.0.0.5:5432"
    Then the output says the broader raw deny already blocks it
    And "lns-policy.yaml" has exactly one raw rule for "10.0.0.0/8:5432"

  Scenario: Annotating a raw rule the file already holds edits it in place
    Given "lns-policy.yaml" has a raw allow rule for "db.internal:5432"
    When the developer allows the raw destination "db.internal:5432" with description "project database"
    Then "lns-policy.yaml" has exactly one raw rule for "db.internal:5432"
    And the output says the description was updated

  # A copy of the grant stranded behind the rule that pre-empts it is the same rule,
  # not a second one to keep: moving it into force is the whole of what was asked for,
  # and leaving the old line behind grows the file with rules the gate never reaches.
  Scenario: A raw allow the file holds but a broader rule pre-empts is moved in front of it
    Given "lns-policy.yaml" has a raw allow rule for "10.0.0.0/24:5432" ahead of a raw allow rule for "10.0.0.5:5432"
    When the developer allows the raw destination "10.0.0.5:5432"
    Then the raw allow rule for "10.0.0.5:5432" sits ahead of the raw rule for "10.0.0.0/24:5432"
    And "lns-policy.yaml" has exactly one raw allow rule for "10.0.0.5:5432"

  Scenario: Moving a stranded raw rule into force keeps the note it was carrying
    Given "lns-policy.yaml" has a raw allow rule for "10.0.0.0/24:5432" ahead of a raw allow rule for "10.0.0.5:5432" described as "project database"
    When the developer allows the raw destination "10.0.0.5:5432"
    Then "lns-policy.yaml" describes the raw allow rule for "10.0.0.5:5432" as "project database"
    And "lns-policy.yaml" has exactly one raw allow rule for "10.0.0.5:5432"

  Scenario: Adding the same raw rule twice reports it rather than duplicating it
    Given "lns-policy.yaml" has a raw allow rule for "db.internal:5432"
    When the developer allows the raw destination "db.internal:5432"
    Then the output says the raw rule is already present
    And "lns-policy.yaml" has exactly one raw rule for "db.internal:5432"

  # Nothing between the workload and a raw destination can read the traffic, so naming
  # the one caller that may open it is the narrowest the grant gets — and the flag is on
  # the verbs whose scoping changes the outcome, which a deny's never does.
  Scenario: Scoping a raw allow to one binary records it in the policy file
    When the developer allows the raw destination "db.internal:5432" scoped to "/usr/bin/psql"
    Then "lns-policy.yaml" contains a raw allow rule for "db.internal:5432" scoped to "/usr/bin/psql"
    And the output says every other caller is now denied "db.internal:5432"

  Scenario: A relative binary path is refused before any raw rule is written
    Given the developer is in a directory with no "lns-policy.yaml"
    When the developer allows the raw destination "db.internal:5432" scoped to "psql"
    Then the command fails with an exit code other than 0
    And "./lns-policy.yaml" is not created

  # Narrowing who may open a destination the file already splices for everyone only takes
  # effect in front of that rule, since the gate stops at the first match.
  Scenario: Narrowing a raw destination an open raw rule already splices puts the scoped rule first
    Given "lns-policy.yaml" has a raw allow rule for "db.internal:5432"
    When the developer allows the raw destination "db.internal:5432" scoped to "/usr/bin/psql"
    Then "lns-policy.yaml" lists the raw rule scoped to "/usr/bin/psql" before the raw rule for "db.internal:5432"
    And the output says every other caller is now denied "db.internal:5432"

  # The raw table is consulted before the HTTP one, so a raw rule takes its destination
  # over from every HTTP rule naming that host on that port. Lifting a deny that way is
  # a widening of the file's own block, which is the developer's call to make, not ours.
  Scenario: A raw allow that would lift an existing HTTP deny is refused
    Given "lns-policy.yaml" has a deny rule for "db.internal"
    When the developer allows the raw destination "db.internal:5432"
    Then the command fails with an exit code other than 0
    And the error explains the HTTP deny would stop applying
    And "lns-policy.yaml" no longer contains a raw rule for "db.internal:5432"

  # The refusal is about what writing the rule would change; a rule the file already
  # holds changes nothing, and the deny it displaces is already displaced.
  Scenario: Re-adding a raw allow the file already holds is not refused by the deny it already pre-empts
    Given "lns-policy.yaml" has a raw allow rule for "db.internal:5432" and a deny rule for "db.internal"
    When the developer allows the raw destination "db.internal:5432"
    Then the command succeeds
    And the output says the raw rule is already present

  Scenario: A raw allow says which HTTP rule it takes the destination over from
    Given "lns-policy.yaml" has an allow rule for "db.internal"
    When the developer allows the raw destination "db.internal:5432"
    Then "lns-policy.yaml" contains a raw allow rule for "db.internal:5432"
    And the output says the HTTP rule for "db.internal" no longer applies

  Scenario: Listing rules shows which table each rule is in
    Given "lns-policy.yaml" has an allow rule for "api.linear.app" and a raw allow rule for "db.internal:5432"
    When the developer lists rules
    Then the output shows "api.linear.app" in the "http" table and "db.internal:5432" in the "tcp" table

  Scenario: Removing a raw rule by pattern deletes it from the raw table
    Given "lns-policy.yaml" has an allow rule for "api.linear.app" and a raw allow rule for "db.internal:5432"
    When the developer removes the rule matching "db.internal:5432"
    Then "lns-policy.yaml" no longer contains a raw rule for "db.internal:5432"
    And "lns-policy.yaml" contains an allow rule for "api.linear.app"

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
