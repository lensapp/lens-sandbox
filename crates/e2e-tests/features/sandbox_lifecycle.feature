Feature: sandbox lifecycle verbs reach the service end to end
  The lns sandbox family talks to the real lns-service over its Unix
  socket. Without a bootable microVM these scenarios confirm the wiring:
  requests arrive, are dispatched, and the daemon's answers come back.
  Happy paths against live runs are covered by the behaviours layers.

  Scenario: the sandbox family lists its verbs in help
    When I run "lns sandbox --help"
    Then the exit code is 0
    And the output contains "stop"
    And the output contains "logs"
    And the output contains "attach"
    And the output contains "inspect"
    And the output contains "stats"

  Scenario: sandbox ls answers with the runs table end to end
    Given the Lens Sandbox service is running
    When I run sandbox command "ls" against the service
    Then the exit code is 0
    And the output contains "ID"

  Scenario: the flat ls alias still reaches the service
    Given the Lens Sandbox service is running
    When I run lns "ls" against the service
    Then the exit code is 0
    And the output contains "ID"

  Scenario: sandbox kill of an unknown run reports the daemon's error
    Given the Lens Sandbox service is running
    When I run sandbox command "kill 4242" against the service
    Then the exit code is non-zero
    And the output contains "4242"

  Scenario: stopping an unknown run reports the daemon's error
    Given the Lens Sandbox service is running
    When I run sandbox command "stop 4242" against the service
    Then the exit code is non-zero
    And the output contains "no such run: 4242"

  Scenario: inspecting an unknown run reports the daemon's error
    Given the Lens Sandbox service is running
    When I run sandbox command "inspect 4242" against the service
    Then the exit code is non-zero
    And the output contains "no such run: 4242"

  Scenario: requesting logs of an unknown run reports the daemon's error
    Given the Lens Sandbox service is running
    When I run sandbox command "logs 4242" against the service
    Then the exit code is non-zero
    And the output contains "no such run: 4242"

  Scenario: attaching to an unknown run reports the daemon's error
    Given the Lens Sandbox service is running
    When I run sandbox command "attach 4242" against the service
    Then the exit code is non-zero
    And the output contains "no such run: 4242"

  Scenario: requesting stats of an unknown run reports the daemon's error
    Given the Lens Sandbox service is running
    When I run sandbox command "stats 4242" against the service
    Then the exit code is non-zero
    And the output contains "4242"

  Scenario: removing an unknown run reports the daemon's error
    Given the Lens Sandbox service is running
    When I run sandbox command "rm 4242" against the service
    Then the exit code is non-zero
    And the output contains "no such run: 4242"

  Scenario: pruning with no finished runs succeeds end to end
    Given the Lens Sandbox service is running
    When I run sandbox command "prune" against the service
    Then the exit code is 0
