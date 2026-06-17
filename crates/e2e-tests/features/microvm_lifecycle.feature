@microvm
Feature: a detached run's lifecycle is driven against a real guest
  A detached run keeps a real microVM alive until it is stopped or killed.
  These scenarios prove the lifecycle verbs reach a booted guest: inspect
  reports the live state, and stop/kill actually tear the VM down and free
  the resources it held. They are imageless — the workload just sleeps via
  the bundled busybox. (logs/stats rendering is pinned at Layer 2, where the
  ring buffer is exercised without a live relay.)

  Scenario: inspecting a live detached run reports it running
    Given the Lens Sandbox service is running
    When the user starts a detached microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox sleep 60'"
    Then the exit code is 0
    When the user inspects that run
    Then the exit code is 0
    And the output contains "running"

  Scenario: stopping a detached run tears down the guest and frees its volume
    Given the Lens Sandbox service is running
    When the user starts a detached microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox sleep 60'" with volume "e2e-vol-stop" at "/data"
    Then the exit code is 0
    When the user stops that run
    Then the exit code is 0
    And volume "e2e-vol-stop" is released

  Scenario: killing a detached run tears down the guest and frees its volume
    Given the Lens Sandbox service is running
    When the user starts a detached microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox sleep 60'" with volume "e2e-vol-kill" at "/data"
    Then the exit code is 0
    When the user kills that run
    Then the exit code is 0
    And volume "e2e-vol-kill" is released
