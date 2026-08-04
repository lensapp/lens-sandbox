Feature: inspecting a typed artifact before running it
  `lns inspect <ref>` is the type-aware, pre-run view of a cached artifact: it
  names the kind, and for a published sandbox it lists the base image, declared
  mounts, ports, and filesets with their mount paths, plus the connectors, and
  it flags an over-broad shipped policy. It lets a consumer review the pieces
  before trusting a configured sandbox.

  Scenario: inspect is discoverable in the front-page help
    When I run "lns inspect --help"
    Then the exit code is 0
    And the output contains "Usage: lns inspect"

  Scenario: inspecting a plain image labels it Image
    Given the service inspects "registry.example.test/some-image:1.0" as a plain image
    When the user runs "lns inspect registry.example.test/some-image:1.0"
    Then the exit code is 0
    And the output contains "Image"

  Scenario: inspecting a sandbox shows its workdir and declared mounts
    Given the service inspects "registry.example.test/some-sandbox:1.0" as a sandbox with launch settings
    When the user runs "lns inspect registry.example.test/some-sandbox:1.0"
    Then the exit code is 0
    And the output contains "workdir: /workspace"
    And the output contains "bind . -> /workspace"
    And the output contains "volume some-cache -> /home/node/.cache"

  Scenario: inspecting a sandbox shows its declared env
    Given the service inspects "registry.example.test/some-sandbox:1.0" as a sandbox setting env "SHELL=/bin/sh"
    When the user runs "lns inspect registry.example.test/some-sandbox:1.0"
    Then the exit code is 0
    And the output contains "env: SHELL=/bin/sh"

  Scenario: inspecting a sandbox shows its declared ports
    Given the service inspects "registry.example.test/some-sandbox:1.0" as a sandbox declaring ports 3003 and 8080:9090
    When the user runs "lns inspect registry.example.test/some-sandbox:1.0"
    Then the exit code is 0
    And the output contains "ports: 3003, 8080:9090"

  Scenario: inspecting a sandbox lists its declared filesets
    Given the service inspects "registry.example.test/some-sandbox:1.0" as a sandbox declaring a fileset at "/root/.agent/skills"
    When the user runs "lns inspect registry.example.test/some-sandbox:1.0"
    Then the exit code is 0
    And the output contains "fileset"
    And the output contains "/root/.agent/skills"

  Scenario: inspecting a published sandbox discloses its credential slots
    Given the service inspects "registry.example.test/some-sandbox:1.0" as a sandbox declaring the "some-provider" credential as "SOME_TOKEN"
    When the user runs "lns inspect registry.example.test/some-sandbox:1.0"
    Then the exit code is 0
    And the output contains "credential: some-provider -> SOME_TOKEN"

  Scenario: inspecting a published sandbox identifies a required credential slot
    Given the service inspects "registry.example.test/some-sandbox:1.0" as a sandbox declaring the required "some-provider" credential as "SOME_TOKEN"
    When the user runs "lns inspect registry.example.test/some-sandbox:1.0"
    Then the exit code is 0
    And the output contains "credential: some-provider -> SOME_TOKEN (required)"

  Scenario: inspecting a sandbox flags a permissive shipped policy
    Given the service inspects "registry.example.test/some-sandbox:1.0" as a sandbox whose policy allows every destination
    When the user runs "lns inspect registry.example.test/some-sandbox:1.0"
    Then the exit code is 0
    And the output contains "wildcard allow"

  Scenario: A pulled sandbox's tools are disclosed before anything runs
    Given a published sandbox declaring tools
    When I run "lns inspect" on its reference
    Then each declared tool and its pinned version is listed
    And the run summary discloses them at launch

  Scenario: inspecting names the host that needs a login
    Given the service reports "inspect" needs a login for host "other-registry.example.test"
    When the user runs "lns inspect some-registry.example/some-sandbox:research"
    Then the exit code is 1
    And the output contains "other-registry.example.test"
