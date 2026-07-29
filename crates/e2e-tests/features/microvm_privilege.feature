@microvm
Feature: the workload runs unprivileged inside a locked-down guest
  The supervisor boots as root, installs the network cage, then drops the
  workload to an unprivileged uid before exec. These scenarios are
  guest-observable: they assert from inside a booted guest that the drop
  happened and that the guest filesystem matches the sandbox invariants
  (a clean /run tmpfs, the lns tooling under /.lens). They are imageless —
  every command is a /bin/sh builtin or the bundled busybox by full path.

  Scenario: the workload process runs as the unprivileged sandbox user, not root
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox id'"
    Then the exit code is 0
    And the output contains "uid=65534"
    And the output does not contain "uid=0"

  Scenario: whoami inside the guest is the sandbox user
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo who=$(/.lens/guest-tools/bin/busybox whoami)'"
    Then the exit code is 0
    And the output contains "who=sandbox"

  Scenario: a root run-as is honoured as an identity
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo uid=$(/.lens/guest-tools/bin/busybox id -u)'" as user "root"
    Then the exit code is 0
    And the output contains "uid=0"

  Scenario: a root run-as keeps HOME and USER
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c 'echo home=[$HOME] user=[$USER]'" as user "root"
    Then the exit code is 0
    And the output does not contain "home=[]"
    And the output does not contain "user=[]"

  Scenario: a root workload cannot tear down the network cage
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c '/.lens/nft flush ruleset'" as user "root"
    Then the exit code is non-zero
    And the output contains "Operation not permitted"

  Scenario: a root workload keeps only the identity-management capabilities
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox grep CapEff /proc/self/status'" as user "root"
    Then the exit code is 0
    And the output contains "00000000000000fb"

  Scenario: an exec lands on the workload's identity, not the broker's root
    Given the Lens Sandbox service is running
    When the user starts a detached microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox sleep 60'"
    Then the exit code is 0
    When the user execs "/bin/sh -c 'echo uid=$(/.lens/guest-tools/bin/busybox id -u)'" in that run
    Then the exit code is 0
    And the output contains "uid=65534"

  Scenario: an exec into a root sandbox is capped like the workload it joins
    Given the Lens Sandbox service is running
    When the user starts a detached microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox sleep 60'" as user "root"
    Then the exit code is 0
    When the user execs "/bin/sh -c '/.lens/guest-tools/bin/busybox grep CapEff /proc/self/status'" in that run
    Then the exit code is 0
    And the output contains "00000000000000fb"

  Scenario: /run is a fresh tmpfs, not the workload's persistent root
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox grep /run /proc/mounts'"
    Then the exit code is 0
    And the output contains "tmpfs"

  Scenario: the lns guest tooling under /.lens is present and executable
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox true && echo tooling-ok-$((7*6))'"
    Then the exit code is 0
    And the output contains "tooling-ok-42"

