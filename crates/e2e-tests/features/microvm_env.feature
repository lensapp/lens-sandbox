@microvm
Feature: user env reaches the workload inside a real guest
  `-e KEY=VALUE` injects a non-secret environment variable that the workload
  must see at runtime. These are guest-observable: the command reads the
  variable back from inside the booted guest. Imageless — the value is read
  with the bundled busybox `printenv`.

  Scenario: an injected variable is visible to the workload
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox printenv E2E_VAR'" with env "E2E_VAR=injected-value-123"
    Then the exit code is 0
    And the output contains "injected-value-123"

  Scenario: a value containing '=' is preserved past the first separator
    Given the Lens Sandbox service is running
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox printenv E2E_KV'" with env "E2E_KV=a=b=c"
    Then the exit code is 0
    And the output contains "a=b=c"
