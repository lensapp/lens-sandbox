Feature: running a sandbox
  `lns run` always targets a sandbox — never a raw OCI image. A REF is a
  registry coordinate or a local `lns.yaml`; omitting it runs `./lns.yaml`.
  Running sandboxes are listed by `lns ps`.

  Scenario: run targets a sandbox reference
    Given the registry serves the sandbox "ghcr.io/team/hermes:1.4.0"
    When the user runs "lns run ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the service received a request to run a sandbox

  Scenario: run refuses a plain image reference
    Given the reference "alpine:3.20" is a plain OCI image, not a sandbox
    When the user runs "lns run alpine:3.20"
    Then the command fails with an exit code other than 0
    And the output contains "not a sandbox"
    And the output contains "lns init"

  Scenario: run with no reference runs the local lns.yaml
    Given a valid lns.yaml in the current directory
    When the user runs "lns run"
    Then the exit code is 0
    And the service received a request to run a sandbox

  Scenario: run with no reference and no lns.yaml fails clearly
    Given the current directory has no lns.yaml
    When the user runs "lns run"
    Then the command fails with an exit code other than 0
    And the output contains "no lns.yaml"
    And the output contains "lns init"

  Scenario: run --policy applies a per-run policy override
    Given the registry serves the sandbox "ghcr.io/team/hermes:1.4.0"
    When the user runs "lns run --policy strict.yaml ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the run summary names "strict.yaml" as the policy source

  Scenario: ps lists running sandboxes with cpu and memory
    Given the service reports one running sandbox using 125 permille cpu and 92274688 bytes
    When the user runs "lns ps"
    Then the exit code is 0
    And the output contains "CPU"
    And the output contains "MEM"
    And the output contains "12.5%"
