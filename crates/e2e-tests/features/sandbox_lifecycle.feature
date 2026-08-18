Feature: sandbox lifecycle verbs reach the service end to end
  The lns sandbox family talks to the real lns-service over its Unix
  socket. Without a bootable microVM these scenarios confirm the wiring:
  requests arrive, are dispatched, and the daemon's answers come back.
  Happy paths against live runs are covered by the behaviours layers, and
  the cached-store verbs (ls, rmi, prune) by sandbox_management.

  Scenario: the sandbox family lists its verbs in help
    When I run "lns sandbox --help"
    Then the exit code is 0
    And the output contains "stop"
    And the output contains "logs"
    And the output contains "attach"
    And the output contains "inspect"

  Scenario: the top-level surface lists the sandbox shortcuts
    When I run "lns --help"
    Then the exit code is 0
    And the output contains "init"
    And the output contains "artifact"
    And the output contains "ps"
    And the output contains "push"
    And the output contains "pull"
    And the output contains "tag"
    And the output contains "stop"
    And the output contains "start"
    And the output contains "rm"
    And the output contains "rmi"
    And the output contains "inspect"
    And the output contains "logs"
    And the output contains "attach"
    And the output contains "kill"

  Scenario: sandbox kill of an unknown run reports the daemon's error
    Given the Lens Sandbox service is running
    When I run sandbox command "kill 4242" against the service
    Then the exit code is non-zero
    And the output contains "4242"

  Scenario: kill accepts a --signal override while reporting an unknown run
    Given the Lens Sandbox service is running
    When I run lns "kill --signal KILL 4242" against the service
    Then the exit code is non-zero
    And the output contains "4242"

  Scenario: kill rejects an unsupported signal name
    Given the Lens Sandbox service is running
    When I run lns "kill --signal USR1 4242" against the service
    Then the exit code is non-zero
    And the output contains "USR1"

  Scenario: ps renders only its header when nothing is running
    Given the Lens Sandbox service is running
    When I run lns "ps" against the service
    Then the exit code is 0
    And the output contains "CPU"
    And the output contains "MEM"

  Scenario: stopping an unknown run reports the daemon's error
    Given the Lens Sandbox service is running
    When I run sandbox command "stop 4242" against the service
    Then the exit code is non-zero
    And the output contains "no such run: 4242"

  Scenario: stop accepts a --timeout override while reporting an unknown run
    Given the Lens Sandbox service is running
    When I run lns "stop --timeout 1 4242" against the service
    Then the exit code is non-zero
    And the output contains "no such run: 4242"

  Scenario: requesting logs of an unknown run reports the daemon's error
    Given the Lens Sandbox service is running
    When I run sandbox command "logs 4242" against the service
    Then the exit code is non-zero
    And the output contains "no such run: 4242"

  Scenario: logs -f of an unknown run reports the daemon's error
    Given the Lens Sandbox service is running
    When I run lns "logs -f 4242" against the service
    Then the exit code is non-zero
    And the output contains "no such run: 4242"

  Scenario: attaching to an unknown run reports the daemon's error
    Given the Lens Sandbox service is running
    When I run sandbox command "attach 4242" against the service
    Then the exit code is non-zero
    And the output contains "no such run: 4242"

  Scenario: attach accepts a custom detach chord while reporting an unknown run
    Given the Lens Sandbox service is running
    When I run lns "attach --detach-keys ctrl-a,ctrl-b 4242" against the service
    Then the exit code is non-zero
    And the output contains "no such run: 4242"

  Scenario: attach rejects an unparseable detach chord
    When I run lns "attach --detach-keys bogus 4242" against the service
    Then the exit code is non-zero
    And the output contains "bogus"
