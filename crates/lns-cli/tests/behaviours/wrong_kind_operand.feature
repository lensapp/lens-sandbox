Feature: a RUN verb given a document says which command takes it
  §2.4: a command takes a REF or a RUN, never either. Given the wrong
  kind, it says which it wanted and which command takes the one you
  typed. A path names a document, and a document is never a RUN, so
  the sandbox verbs redirect to `lns artifact inspect` — the command
  that reads one — instead of asking the service for a run that
  cannot exist.

  Scenario: inspect redirects a path-shaped operand to the artifact namespace
    When the user runs sandbox command "inspect ./lns.yaml"
    Then the exit code is 2
    And the output contains "takes a RUN"
    And the output contains "lns artifact inspect"

  Scenario: rm says a document is not a sandbox
    When the user runs sandbox command "rm ./lns.dev.yaml"
    Then the exit code is 2
    And the output contains "takes a RUN"
    And the output contains "lns artifact inspect"

  Scenario: logs has no artifact counterpart, and still names the way out
    When the user runs sandbox command "logs ."
    Then the exit code is 2
    And the output contains "takes a RUN"
    And the output contains "lns artifact inspect"

  Scenario: a name that merely contains a dot is a RUN, not a document
    Given the service will answer an error "no run named v1.2-agent"
    When the user runs sandbox command "inspect v1.2-agent"
    Then the command fails with an exit code other than 0
    And the output contains "no run named v1.2-agent"
    And the output does not contain "takes a RUN"
