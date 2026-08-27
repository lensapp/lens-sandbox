Feature: a wrong-kind operand names the command that takes it
  §2.4: a command takes a REF or a RUN, never either. Given the wrong
  kind, it says which it wanted and which command takes the one you
  typed. A path names a document, and a document is never a RUN, so
  the sandbox verbs redirect to `lns artifact inspect` — the command
  that reads one — instead of asking the service for a run that
  cannot exist. A registry coordinate is not a path, so the service
  answers it, and §6 renders that answer as one sentence in the
  product's own words.

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

  Scenario: stop given a registry coordinate names the artifact namespace
    Given the service will answer an error "no such run: ghcr.io/team/x:1.0"
    When the user runs sandbox command "stop ghcr.io/team/x:1.0"
    Then the command fails with an exit code other than 0
    And the output contains "no such sandbox: ghcr.io/team/x:1.0"
    And the output contains "that looks like an artifact reference"
    And the output contains "lns artifact inspect"
    And the output does not contain "daemon error"

  Scenario: rm given a registry coordinate names the artifact verb that removes one
    Given the service will answer a RemoveRun error "no such run: ghcr.io/team/x:1.0"
    When the user runs sandbox command "rm ghcr.io/team/x:1.0"
    Then the command fails with an exit code other than 0
    And the output contains "no such sandbox: ghcr.io/team/x:1.0"
    And the output contains "lns artifact rm"
    And the output does not contain "daemon error"

  Scenario: a plausible sandbox name gets the miss and nothing else
    Given the service will answer an error "no such run: reviewer"
    When the user runs sandbox command "stop reviewer"
    Then the command fails with an exit code other than 0
    And the output contains "no such sandbox: reviewer"
    And the output does not contain "lns artifact"
    And the output does not contain "daemon error"

  Scenario: an artifact verb given a bare word names the sandbox namespace
    Given the service will answer an error "no such image: hub.lns.run/7:latest"
    When the user runs artifact command "rm 7"
    Then the command fails with an exit code other than 0
    And the output contains "no such artifact: hub.lns.run/7:latest"
    And the output contains "lns sandbox ls"
    And the output does not contain "daemon error"

  Scenario: an artifact verb given a full coordinate says only that it is not cached
    Given the service will answer an error "no such image: ghcr.io/team/x:1.0"
    When the user runs artifact command "rm ghcr.io/team/x:1.0"
    Then the command fails with an exit code other than 0
    And the output contains "no such artifact: ghcr.io/team/x:1.0"
    And the output does not contain "lns sandbox"
    And the output does not contain "daemon error"
