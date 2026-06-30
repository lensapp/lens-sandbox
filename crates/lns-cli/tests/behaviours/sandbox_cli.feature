Feature: managing running sandboxes from the CLI
  `lns sandbox` bundles the lifecycle verbs for runs you have already
  started: stop them gracefully, read or follow their output, re-attach,
  inspect their state, and sample their resource usage.

  Scenario: the sandbox family lists its verbs in help
    When I run "lns sandbox --help"
    Then the exit code is 0
    And the output contains "ls"
    And the output contains "exec"
    And the output contains "kill"
    And the output contains "stop"
    And the output contains "logs"
    And the output contains "attach"
    And the output contains "inspect"
    And the output contains "stats"
    And the output contains "rm"
    And the output contains "rename"
    And the output contains "prune"

  Scenario: the flat ls, exec, and kill verbs stay usable but leave the front page
    When I run "lns --help"
    Then the exit code is 0
    And the output does not contain "docker ps"
    And the output does not contain "docker exec"
    And the output does not contain "docker kill"
    When I run "lns ls"
    Then the exit code is 0
    When I run "lns list"
    Then the exit code is 0
    When I run "lns kill 3"
    Then the exit code is 0
    When I run "lns exec 3 -- echo hi"
    Then the exit code is 0

  Scenario: sandbox ls renders the runs table
    Given the service reports a run listing with run 3 of image "some-image" running
    When the user runs sandbox command "ls"
    Then the exit code is 0
    And the output contains "ID"
    And the output contains "some-image"
    And the output contains "running"
    And the service received a ListRuns request

  Scenario: sandbox list is an alias for sandbox ls
    Given the service reports a run listing with run 3 of image "some-image" running
    When the user runs sandbox command "list"
    Then the exit code is 0
    And the output contains "ID"
    And the output contains "some-image"
    And the output contains "running"
    And the service received a ListRuns request

  Scenario: sandbox kill sends the requested signal
    Given the service will answer Acknowledged
    When the user runs sandbox command "kill 3 --signal KILL"
    Then the exit code is 0
    And the output contains "killed run 3"
    And the service received a Kill request for run 3 with signal KILL

  Scenario: sandbox kill rejects an unknown signal name
    Given the service will answer Acknowledged
    When the user runs sandbox command "kill 3 --signal NOPE"
    Then the command fails with an exit code other than 0
    And the output contains "unknown signal"

  Scenario: stopping a run reports a graceful stop
    Given the service will answer RunStopped without force
    When the user runs sandbox command "stop 3"
    Then the exit code is 0
    And the output contains "stopped run 3"
    And the service received a StopRun request for run 3 with timeout 10

  Scenario: a custom stop timeout is forwarded to the service
    Given the service will answer RunStopped without force
    When the user runs sandbox command "stop 3 --timeout 5"
    Then the service received a StopRun request for run 3 with timeout 5

  Scenario: a stop that had to escalate reports the kill
    Given the service will answer RunStopped with force
    When the user runs sandbox command "stop 3"
    Then the exit code is 0
    And the output contains "killed run 3"

  Scenario: stopping an unknown run fails with the daemon's reason
    Given the service will answer an error "no active run with id 99"
    When the user runs sandbox command "stop 99"
    Then the command fails with an exit code other than 0
    And the output contains "no active run with id 99"

  Scenario: the sandbox verbs surface an unreachable service
    Given the sandbox service is unreachable
    When the user runs sandbox command "stop 3"
    Then the command fails with an exit code other than 0
    And the output contains "is it running?"

  Scenario: inspecting a run prints its state and configuration as JSON
    Given the service reports run 3 of image "some-image:1" running with 2 cpus and 1024 MiB
    When the user runs sandbox command "inspect 3"
    Then the exit code is 0
    And the output contains "some-image:1"
    And the output contains "running"
    And the output contains "memMib"

  Scenario: inspect embeds the policy file when it is readable
    Given the service reports run 3 with policy path "/work/lns-policy.yaml"
    And the policy file parses with default verdict "ask"
    When the user runs sandbox command "inspect 3"
    Then the exit code is 0
    And the output contains "defaultVerdict"
    And the output contains "/work/lns-policy.yaml"

  Scenario: inspect marks an unreadable policy file instead of failing
    Given the service reports run 3 with policy path "/work/lns-policy.yaml"
    When the user runs sandbox command "inspect 3"
    Then the exit code is 0
    And the output contains "policy file could not be read"

  Scenario: stats renders the sampled cpu share and memory
    Given the service reports run 3 using 125 permille cpu and 92274688 of 536870912 bytes
    When the user runs sandbox command "stats 3"
    Then the exit code is 0
    And the output contains "12.5%"
    And the output contains "88.0 MiB / 512.0 MiB"

  Scenario: logs dumps the captured output and stops at the end of the buffer
    Given the run 3 stream carries stdout "hello from the workload" then ends
    When the user runs sandbox command "logs 3"
    Then the exit code is 0
    And the workload stdout contains "hello from the workload"
    And the service received a RunLogs request for run 3 without follow

  Scenario: logs -f asks the service to follow until the run exits
    Given the run 3 stream carries stdout "tick" then exits with code 0
    When the user runs sandbox command "logs -f 3"
    Then the exit code is 0
    And the service received a RunLogs request for run 3 with follow

  Scenario: logs of an unknown run fails with the daemon's reason
    Given the run 99 stream opens with error "no active run with id 99"
    When the user runs sandbox command "logs 99"
    Then the command fails with an exit code other than 0
    And the output contains "no active run with id 99"

  Scenario: attach relays live output and adopts the workload's exit code
    Given the run 3 stream carries stdout "live output" then exits with code 5
    When the user runs sandbox command "attach 3"
    Then the exit code is 5
    And the workload stdout contains "live output"
    And the service received an AttachRun request for run 3

  Scenario: rm drops a finished run from the list
    Given the service will answer Acknowledged
    When the user runs sandbox command "rm 3"
    Then the exit code is 0
    And the output contains "removed run 3"
    And the service received a RemoveRun request for run 3

  Scenario: rm of a still-running run fails with the daemon's reason
    Given the service will answer an error "run 3 is still running; stop it first with `lns sandbox stop 3`"
    When the user runs sandbox command "rm 3"
    Then the command fails with an exit code other than 0
    And the output contains "still running"

  Scenario: prune removes every finished run and lists them
    Given the service will answer RunsPruned for runs 4 and 7
    When the user runs sandbox command "prune"
    Then the exit code is 0
    And the output contains "removed run 000000040000"
    And the output contains "removed run 000000070000"
    And the service received a PruneRuns request

  Scenario: prune reports when there is nothing to remove
    Given the service will answer RunsPruned for no runs
    When the user runs sandbox command "prune"
    Then the exit code is 0
    And the output contains "no finished runs to remove"
