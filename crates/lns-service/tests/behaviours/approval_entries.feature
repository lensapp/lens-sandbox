Feature: the approvals a run keeps
  Every question the approval window raises, and every notice it shows,
  outlives its card. lns-service records it as an approval entry in the
  run's own directory, beside decisions.yaml, so a card the developer
  closes, misses, or answers in haste is still there to answer, or to
  answer again. The newest answer on an entry replaces the one before
  it. The gate does not change: a request nothing decides in time still
  fails closed, and an answer given after the request has gone writes
  the rule without replaying the call. A card can be raised for a
  destination the run has already answered: the policy frame reaches the
  guest a moment after the answer is written, and a connector hold asks
  about a destination for as long as the connector is undecided. Such a
  card is listed, and it leaves the answer the entry already has.

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

  # The frame reaches the guest a moment after the answer is written, so a
  # request already in flight raises a card the answer has already decided.
  Scenario: A card raised while the answer travels leaves the answer the entry has
    Given the run records what it decides in "decisions.yaml"
    And the run's approvals list "api.linear.app" as always allowed
    When a workload reaches "api.linear.app"
    Then the run's approvals list "api.linear.app" as always allowed

  # A once verdict answers the request in hand and earns no rule, so it changes
  # nothing about what the run has decided. Reading the entry back as undecided
  # would say the destination is open to question while the rule still allows it.
  Scenario: A once verdict on a card the entry already answered leaves the answer it has
    Given the run records what it decides in "decisions.yaml"
    And the run's approvals list "api.linear.app" as always allowed
    When a workload reaches "api.linear.app"
    And the developer picks "allow once"
    Then the run's approvals list "api.linear.app" as always allowed

  # The other way to earn no rule: the answer the entry already gave shadows
  # the one the developer just picked, so nothing is written and what the run
  # decides is still the answer the entry carries.
  Scenario: An always verdict refused as shadowed leaves the answer the entry has
    Given the run records what it decides in "decisions.yaml"
    And the run's approvals list "api.linear.app" as always allowed
    When a workload reaches "api.linear.app"
    And the developer picks "always deny"
    Then the run's approvals list "api.linear.app" as always allowed

  # `rm` clears the record and keeps the rule, and a connector hold re-asks
  # about a destination the policy already allows. Both leave a rule with no
  # entry beside it, and the card the run raises is still one to list.
  Scenario: A run that holds the rule but no record of the question lists the card it raises
    Given the run records what it decides in "decisions.yaml"
    And the run's approvals list "api.linear.app" as always allowed
    And the developer removes that entry
    When a workload reaches "api.linear.app"
    Then the run's approvals list "api.linear.app" as undecided

  Scenario: The run's approvals survive a service restart
    Given the run's approvals list "api.linear.app" as undecided
    When lns-service restarts
    Then the run's approvals still list "api.linear.app" as undecided

  # A restarted service holds no record of the runs the one before it started,
  # and the entries are in the runs' own directories. The list reads them from
  # there, or a restart hides every question a developer has not answered yet.
  Scenario: A run this service holds no record of is still listed
    Given a stopped sandbox whose approvals list "api.linear.app" as undecided
    And this service holds no record of that sandbox
    When the service lists what every run was asked
    Then the list holds "api.linear.app"
    And the list names the sandbox by its id

  Scenario: A run this service holds no record of is answerable by its id
    Given a stopped sandbox whose approvals list "api.linear.app" as undecided
    And this service holds no record of that sandbox
    When the developer answers "always allow" on that entry through the service
    Then that sandbox's "decisions.yaml" contains a new allow rule for "api.linear.app"

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

  Scenario: A notice is cleared from the list
    Given an approval card is visible for a request to "api.linear.app"
    And the policy file cannot be written
    When the developer picks "always allow"
    And the developer removes that notice
    Then the run's approvals hold no notice
    And the run's approvals still list "api.linear.app" as always allowed

  # Removing a line removes the record of the question. What it decided stays
  # decided: the developer is clearing a notification, not an answer.
  Scenario: An answered question is cleared, and stays decided
    Given the run records what it decides in "decisions.yaml"
    And the run's approvals list "api.linear.app" as always allowed
    When the developer removes that entry
    Then the run's approvals hold nothing about "api.linear.app"
    And "decisions.yaml" contains an allow rule for "api.linear.app"

  Scenario: A granted connector is cleared, and stays granted
    Given the developer grants the connector "linear" to the run
    When the developer removes the connector entry
    Then the run's approvals hold nothing about "linear"
    And the run still supplies what the connector "linear" granted

  # A connector card is a question like any other: closing it must leave the
  # offer listed, or the one card the developer cannot get back is this one.
  Scenario: A connector card is listed from the moment it is raised
    Given a connector "linear" serves "api.linear.app"
    When a workload reaches "api.linear.app"
    Then the run's approvals list the connector "linear" as undecided

  Scenario: Answering the connector card later replaces the offer it left
    Given a connector "linear" serves "api.linear.app"
    And a workload reaches "api.linear.app"
    When the developer grants the connector "linear" to the run
    Then the run's approvals list the connector "linear" as granted
