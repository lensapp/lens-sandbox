Feature: a RUN verb given a document redirects before touching the service

  §2.4: given the wrong kind of operand, a command says which it wanted
  and which command takes the one you typed. The redirect is a client-side
  decision, so it answers 2 even when no service is running — never 125.

  Scenario: lns exec given a document names the artifact reader instead
    Given a clean lns cache home
    And no Lens Sandbox service is running
    When I run "lns exec ./lns.yaml -- true"
    Then the exit code is 2
    And the output contains "takes a RUN"
    And the output contains "lns artifact inspect"

  Scenario: lns sandbox logs given a document is the same refusal
    Given a clean lns cache home
    And no Lens Sandbox service is running
    When I run "lns sandbox logs ./lns.yaml"
    Then the exit code is 2
    And the output contains "lns artifact inspect"
