@microvm
Feature: user env reaches the workload inside a real guest
  `-e KEY=VALUE` injects a non-secret environment variable that the workload
  must see at runtime. These are guest-observable: the command reads the
  variable back from inside the booted guest. Imageless — the value is read
  with the bundled busybox `printenv`.

  Scenario: the definition's command and env run in the guest
    Given the LNS service is running
    And the project definition sets env "MODE=research"
    And the project definition sets command "/bin/sh -c '/.lens/guest-tools/bin/busybox printenv MODE'"
    When the user runs the sandbox definition
    Then the exit code is 0
    And the output contains "research"

  Scenario: an explicit command and -e still win over the definition's
    Given the LNS service is running
    And the project definition sets env "MODE=research"
    And the project definition sets command "/bin/false"
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox printenv MODE'" with env "MODE=prod"
    Then the exit code is 0
    And the output contains "prod"

  Scenario: an injected variable is visible to the workload
    Given the LNS service is running
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox printenv E2E_VAR'" with env "E2E_VAR=injected-value-123"
    Then the exit code is 0
    And the output contains "injected-value-123"

  Scenario: a value containing '=' is preserved past the first separator
    Given the LNS service is running
    When the user runs a microVM command "/bin/sh -c '/.lens/guest-tools/bin/busybox printenv E2E_KV'" with env "E2E_KV=a=b=c"
    Then the exit code is 0
    And the output contains "a=b=c"
