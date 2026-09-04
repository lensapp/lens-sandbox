Feature: reading and answering approvals from the CLI
  `lns approval` is the terminal surface for what the approval window
  asks. It lists what each run has been asked and what the developer
  answered, and it answers an entry — for the first time, or again —
  without the window. The service owns the entries, so every verb is a
  thin IPC call.

  Scenario: the approval family lists its verbs in help
    When I run "lns approval --help"
    Then the exit code is 0
    And the output contains "ls"
    And the output contains "answer"

  Scenario: a bare approval invocation surfaces usage
    When I run "lns approval"
    Then the exit code is 2
    And the output contains "Usage: lns approval"

  Scenario: listing approvals renders the sandbox, what was asked, and the answer
    Given the service reports an undecided approval "a1" for "api.linear.app" raised by "reviewer"
    And the service reports an approval "a2" for "api.github.com" raised by "reviewer" answered always allow
    When the user runs approval command "ls"
    Then the exit code is 0
    And the output contains "SANDBOX"
    And the output contains "ASKED ABOUT"
    And the output contains "ANSWER"
    And the output contains "api.linear.app"
    And the output contains "undecided"
    And the output contains "api.github.com"
    And the output contains "always allow"

  Scenario: the developer answers a pending question at the terminal
    Given the service reports an undecided approval "a1" for "api.linear.app" raised by "reviewer"
    When the user runs approval command "answer a1 always-allow"
    Then the exit code is 0
    And the service is asked to answer "a1" with always allow
    And the output contains "api.linear.app"

  Scenario: the developer reverses an earlier always-decision at the terminal
    Given the service reports an approval "a2" for "api.github.com" raised by "reviewer" answered always allow
    When the user runs approval command "answer a2 ask-again"
    Then the exit code is 0
    And the service is asked to answer "a2" with ask again

  Scenario: the list renders as JSON for a script
    Given the service reports an undecided approval "a1" for "api.linear.app" raised by "reviewer"
    When the user runs approval command "ls --format json"
    Then the exit code is 0
    And the output is valid JSON
    And the JSON output contains "api.linear.app"
    And the JSON output contains "destination"

  Scenario: an unknown entry id fails with an error naming the id
    Given the service reports no approval "a9"
    When the user runs approval command "answer a9 always-allow"
    Then the exit code is 1
    And the output contains "a9"

  Scenario: the developer denies a destination for good
    Given the service reports an undecided approval "a1" for "api.linear.app" raised by "reviewer"
    When the user runs approval command "answer a1 always-deny"
    Then the exit code is 0
    And the service is asked to answer "a1" with always deny

  Scenario: an answer the service could not write says why
    Given the service reports an undecided approval "a1" for "api.linear.app" raised by "reviewer"
    And the service will not write the rule, saying "the rule for \"*.linear.app\" already decides this destination"
    When the user runs approval command "answer a1 always-allow"
    Then the exit code is 1
    And the output contains "already decides this destination"

  Scenario: listing a sandbox the service does not know fails with the name
    Given the service knows no sandbox "ghost"
    When the user runs approval command "ls ghost"
    Then the exit code is 1
    And the output contains "ghost"

  Scenario: a service refusal is surfaced as it was given
    Given the service refuses approvals with "lns home is not writable"
    When the user runs approval command "ls"
    Then the exit code is 1
    And the output contains "lns home is not writable"

  Scenario: an answer the service does not recognise is reported, not swallowed
    Given the service answers approvals with something else
    When the user runs approval command "ls"
    Then the exit code is 1
    And the output contains "unexpected response"

  Scenario: an unrecognised answer to an answer is reported too
    Given the service reports an undecided approval "a1" for "api.linear.app" raised by "reviewer"
    And the service answers approvals with something else
    When the user runs approval command "answer a1 always-allow"
    Then the exit code is 1
    And the output contains "unexpected response"

  Scenario: the list filters to one sandbox
    Given the service reports an undecided approval "a1" for "api.linear.app" raised by "reviewer"
    And the service reports an undecided approval "a3" for "api.stripe.com" raised by "auditor"
    When the user runs approval command "ls reviewer"
    Then the exit code is 0
    And the output contains "api.linear.app"
    And the output does not contain "api.stripe.com"
