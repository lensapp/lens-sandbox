@microvm
Feature: credential placeholders cross the boundary as fakes, never real secrets
  Real secrets stay outside the workload: the guest is seeded with credential-
  shaped placeholders, and a real value is only ever swapped in at the network
  boundary after an explicit decision. This scenario is guest-observable — it
  reads the workload's own environment from inside a booted guest and asserts
  every seeded credential carries the synthetic placeholder marker, so nothing
  real has leaked in. The arming-at-the-boundary decision flow is interactive
  and pinned at Layer 2.

  Scenario: seeded credential values are placeholders, carrying no real secret
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox env'"
    Then the exit code is 0
    And the output contains "LNSPLACEHOLDER"

  Scenario: a definition-declared integration arms in the guest without a connect
    Given a clean lns cache home
    And the home's integration catalog declares "some-provider" managing "SOME_TOKEN"
    And the Lens Sandbox service is running in that home
    And the project definition declares integration "some-provider"
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox env'"
    Then the exit code is 0
    And the output contains "SOME_TOKEN=some-provider-LNSPLACEHOLDER"
