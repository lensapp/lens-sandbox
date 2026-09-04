Feature: the approvals a run keeps
  Every question the approval window raises, and every notice it shows,
  outlives its card. lns-service records it as an approval entry in the
  run's own directory, beside decisions.yaml, so a card the developer
  closes, misses, or answers in haste is still there to answer, or to
  answer again. The newest answer on an entry replaces the one before
  it. The gate does not change: a request nothing decides in time still
  fails closed, and an answer given after the request has gone writes
  the rule without replaying the call. A request an existing rule
  decides never reaches this layer — the guest's own gate stops at the
  first matching rule — so no entry is possible for one.

  Scenario: A closed card stays in the run's approvals as undecided
    Given a workload is running in the sandbox
    And an approval card is visible for a request to "api.linear.app"
    When the developer closes the card without choosing
    Then the workload's request is failed at the boundary as undecided
    And the run's approvals list "api.linear.app" as undecided
    And the audit chain records no approval for "api.linear.app"

  Scenario: A timed-out card stays in the run's approvals as undecided
    Given an approval card is visible for a request to "api.linear.app"
    When no decision is recorded before the configured approval timeout
    Then the workload's request is failed at the boundary as undecided
    And the approval card is removed from the approval window
    And the run's approvals list "api.linear.app" as undecided

  Scenario: A card withdrawn by a workload exit stays in the run's approvals
    Given a workload has an open approval card for "api.linear.app"
    When the workload exits before a decision is recorded
    Then the approval card is removed from the approval window
    And the run's approvals list "api.linear.app" as withdrawn

  Scenario: Every entry names the sandbox that raised it
    Given the sandbox is named "reviewer"
    And an approval card is visible for a request to "api.linear.app"
    When the developer closes the card without choosing
    Then the entry for "api.linear.app" names the sandbox "reviewer"

  Scenario: An answered card stays in the run's approvals with the answer it got
    Given the run records what it decides in "decisions.yaml"
    And an approval card is visible for a request to "api.linear.app"
    When the developer picks "always allow"
    Then the run's approvals list "api.linear.app" as always allowed

  Scenario: A notice the window raises is listed with no verdict to give
    Given an approval card is visible for a request to "api.linear.app"
    And the policy file cannot be written
    When the developer picks "always allow"
    Then the run's approvals hold a notice that the rule could not be persisted
    And that notice offers no verdict

  # Two requests to one destination share one entry, so the second one's fate
  # must not overwrite the answer the developer gave on the first.
  Scenario: A second request timing out leaves the answer the first one got
    Given the run records what it decides in "decisions.yaml"
    And the workload holds two requests to "api.linear.app"
    When the developer picks "always allow" on the first
    And the second request times out
    Then the run's approvals list "api.linear.app" as always allowed

  Scenario: The run's approvals survive a service restart
    Given the run's approvals list "api.linear.app" as undecided
    When lns-service restarts
    Then the run's approvals still list "api.linear.app" as undecided

  Scenario: Removing the sandbox removes its approvals
    Given a stopped sandbox whose approvals list "api.linear.app" as undecided
    When the developer removes the sandbox
    Then the run directory holds no approvals

  Scenario: Answering an undecided entry writes the rule and hot-swaps the policy
    Given the run records what it decides in "decisions.yaml"
    And the run's approvals list "api.linear.app" as undecided
    When the developer answers "always allow" on that entry
    Then "decisions.yaml" contains a new allow rule for "api.linear.app"
    And the running policy contains the same rule
    And a future request to "api.linear.app" is allowed without prompting

  Scenario: A settled entry offers the always verdicts and asking again
    Given the run's approvals list "api.linear.app" as always allowed
    When the developer reads that entry
    Then the entry offers "always allow", "always deny", and "ask again"
    And the entry offers no once verdict

  Scenario: Re-answering an entry with the other verdict rewrites the rule
    Given the run records what it decides in "decisions.yaml"
    And the run's approvals list "api.linear.app" as always allowed
    When the developer answers "always deny" on that entry
    Then "decisions.yaml" contains a deny rule for "api.linear.app"
    And "decisions.yaml" contains no allow rule for "api.linear.app"
    And the running policy contains the same rule
    And the run's approvals list "api.linear.app" as always denied

  Scenario: Asking again withdraws the rule the entry wrote
    Given the run records what it decides in "decisions.yaml"
    And the run's approvals list "api.linear.app" as always allowed
    When the developer answers "ask again" on that entry
    Then "decisions.yaml" contains no rule for "api.linear.app"
    And the running policy contains no rule for "api.linear.app"
    And the run's approvals list "api.linear.app" as undecided
    And a future request to "api.linear.app" prompts again

  Scenario: An entry of a stopped sandbox is answerable and edits its decisions file
    Given a stopped sandbox whose approvals list "api.linear.app" as undecided
    When the developer answers "always allow" on that entry
    Then that sandbox's "decisions.yaml" contains a new allow rule for "api.linear.app"
    And the sandbox stays stopped

  Scenario: A connector the run granted is listed as granted
    Given the developer grants the connector "linear" to the run
    Then the run's approvals list the connector "linear" as granted
    And that entry offers no verdict
