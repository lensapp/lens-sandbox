Feature: managing running sandboxes from the CLI
  `lns sandbox` groups the lifecycle verbs for runs you have already
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
    And the output contains "rm"

  Scenario: the sandbox help describes the verbs in the product's own words
    When I run "lns sandbox --help"
    Then the exit code is 0
    And the output does not contain "docker" in any casing
    When I run "lns sandbox attach --help"
    Then the exit code is 0
    And the output does not contain "docker" in any casing

  Scenario: the flat exec and kill verbs stay usable but leave the front page
    When I run "lns --help"
    Then the exit code is 0
    And the output does not contain "docker exec"
    And the output does not contain "docker kill"
    When I run "lns kill 3"
    Then the exit code is 0
    When I run "lns exec 3 -- echo hi"
    Then the exit code is 0

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

  Scenario: inspecting a run summarises its state for a reader
    Given the service reports run 3 of image "some-image:1" running with 2 cpus and 1024 MiB
    When the user runs sandbox command "inspect 3"
    Then the exit code is 0
    And the output contains "IMAGE"
    And the output contains "some-image:1"
    And the output contains "running"
    And the output contains "1.0 GiB"

  Scenario: --format json gives a script the whole record the table summarises
    Given the service reports run 3 of image "some-image:1" running with 2 cpus and 1024 MiB
    When the user runs sandbox command "inspect 3 --format json"
    Then the exit code is 0
    And the output contains "some-image:1"
    And the output contains "running"
    And the output contains "memMib"

  Scenario: the record names the two times and invents no third
    Given the service reports run 3 of image "some-image:1" running with 2 cpus and 1024 MiB
    When the user runs sandbox command "inspect 3 --format json"
    Then the exit code is 0
    And the output contains "created"
    And the output contains "started"
    And the output does not contain "uptime"

  Scenario: inspect embeds the run's decisions file when it is readable
    Given the service reports run 3 with policy path "/home/dev/.lns/runs/aa01/decisions.yaml"
    And the policy file parses with one allow rule
    When the user runs sandbox command "inspect 3 --format json"
    Then the exit code is 0
    And the output contains "egress"
    And the output contains "/home/dev/.lns/runs/aa01/decisions.yaml"

  Scenario: inspect marks an unreadable decisions file instead of failing
    Given the service reports run 3 with policy path "/home/dev/.lns/runs/aa01/decisions.yaml"
    When the user runs sandbox command "inspect 3 --format json"
    Then the exit code is 0
    And the output contains "policy file could not be read"

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

  Scenario: save writes the run out as a document at the path the user named
    Given the service renders run 3 as "apiVersion: lns.run/v1\nkind: sandbox\nname: out\n"
    When the user runs sandbox command "save 3 -f ./out.yaml"
    Then the exit code is 0
    And the document written to "./out.yaml" contains "kind: sandbox"
    And the output contains "saved sandbox 3"

  Scenario: save names the document after the file it lands in
    Given the service renders run 3 as "apiVersion: lns.run/v1\nkind: mixin\nname: agreed\n"
    When the user runs sandbox command "save 3 -f ./agreed.yaml --kind mixin"
    Then the exit code is 0
    And the service received a SaveRun request for run 3 naming the document "agreed"

  Scenario: save refuses to overwrite a file that is already there
    Given the service renders run 3 as "apiVersion: lns.run/v1\nkind: sandbox\nname: out\n"
    And "./out.yaml" already exists
    When the user runs sandbox command "save 3 -f ./out.yaml"
    Then the command fails with an exit code other than 0
    And the output contains "already exists; not overwriting it"
    And no document was written

  Scenario: save that cannot write names the file the user has to fix
    Given the service renders run 3 as "apiVersion: lns.run/v1\nkind: sandbox\nname: out\n"
    And "./locked/out.yaml" cannot be written
    When the user runs sandbox command "save 3 -f ./locked/out.yaml"
    Then the command fails with an exit code other than 0
    And the output contains "writing ./locked/out.yaml"
    And no document was written

  Scenario: save without a file to write to is refused by the grammar
    When the grammar is given sandbox command "save 3"
    Then the exit code is 2
    And the output contains "--file"

  Scenario: save into a file whose name a document cannot carry is refused
    When the user runs sandbox command "save 3 -f ./Team_Rules.yaml"
    Then the command fails with an exit code other than 0
    And the output contains "lowercase"
    And no document was written
