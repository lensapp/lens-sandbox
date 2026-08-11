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

  Scenario: a definition-declared connector is not armed in the guest, only offered
    Given a clean lns cache home
    And the home's connector catalog declares "some-provider" managing "SOME_TOKEN"
    And the Lens Sandbox service is running in that home
    And the project definition declares connector "some-provider"
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox env'"
    Then the exit code is 0
    And the output does not contain "SOME_TOKEN=some-provider-LNSPLACEHOLDER"

  Scenario: a declared credential seeds its own placeholder in the guest, not the catalog's
    Given a clean lns cache home
    And the home's connector catalog declares "some-provider" managing "SOME_TOKEN"
    And the Lens Sandbox service is running in that home
    And the project definition declares credential "PROVIDER_KEY" for "api.some-provider.example"
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox env'"
    Then the exit code is 0
    And the output contains "PROVIDER_KEY=lns-placeholder-PROVIDER_KEY"
    And the output does not contain "SOME_TOKEN=some-provider-LNSPLACEHOLDER"
