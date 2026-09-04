@microvm
Feature: the network cage blocks denied egress from a real guest
  The supervisor confines the workload's traffic to a local enforcing proxy
  whose verdict comes from the loaded policy. This proves the enforcement end
  to end from inside a booted guest: under a deny-all policy a real outbound
  connection is refused, and the recorded egress event attributes the attempt
  to the guest client endpoint and owning process. It is guest-observable and
  uses the bundled busybox `wget`. The interactive ask/approve flow needs a
  developer decision and is pinned at Layer 2; this scenario preloads a
  deciding policy so no prompt is raised. The allow-path (a permitted host is
  reachable) is deferred: a
  reliable assertion needs a deterministic host-side endpoint rather than a
  real-internet host, whose reachability makes the test flaky.

  DHCP keepalive needs a manual macOS vmnet check. Start one sandbox and leave
  it running. Start a second sandbox seven minutes later. Confirm that the
  second sandbox gets a network address and can reach an allowed destination.

  Scenario: a deny-all policy blocks the workload's outbound connection
    Given the LNS service is running
    And a network policy that denies all egress
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox wget -T 5 -q -O /dev/null http://1.1.1.1/ && echo reached-$((3*3)) || echo blocked-$((1+1))'"
    Then the exit code is 0
    And the output contains "blocked-2"
    And the output does not contain "reached-9"
    And the audit log for that run records the denied egress with the client endpoint and process
