Feature: inspecting a typed artifact before running it
  `lns inspect <ref>` is the type-aware, pre-run view of a cached artifact: it
  names the kind, and for a published sandbox it lists the base image, declared
  mounts, ports, filesets with their mount paths, and mixins, plus the connectors, and
  it flags an over-broad shipped policy. It lets a consumer review the pieces
  before trusting a configured sandbox.

  Scenario: inspect is discoverable in the front-page help
    When I run "lns inspect --help"
    Then the exit code is 0
    And the output contains "Usage: lns inspect"

  Scenario: inspecting a published mixin labels it mixin and shows every block it carries
    Given the service inspects "ghcr.io/acme/obs-tools:2" as a mixin declaring the tool "node@22.11.0"
    When the user runs "lns inspect ghcr.io/acme/obs-tools:2"
    Then the exit code is 0
    And the output contains "kind: mixin"
    And the output contains "tool: node@22.11.0"
    And the output contains "mixin: ghcr.io/acme/base@sha256:"
    And the output contains "env: MODE=research"
    And the output contains "ports: 9090"
    And the output contains "credential: SOME_TOKEN -> api.some-provider.example"

  Scenario: inspecting a plain image labels it image
    Given the service inspects "registry.example.test/some-image:1.0" as a plain image
    When the user runs "lns inspect registry.example.test/some-image:1.0"
    Then the exit code is 0
    And the output contains "kind: image"

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

  Scenario: inspect discloses the run-as user a pulled sandbox asks for
    Given the service inspects "registry.example.test/some-sandbox:1.0" as a sandbox declaring user "root"
    When the user runs sandbox command "inspect registry.example.test/some-sandbox:1.0"
    Then the exit code is 0
    And the output contains "user: root"

  Scenario: inspecting a published sandbox discloses each credential and where its value may travel
    Given the service inspects "registry.example.test/some-sandbox:1.0" as a sandbox declaring the "SOME_TOKEN" credential for "api.some-provider.example"
    When the user runs "lns inspect registry.example.test/some-sandbox:1.0"
    Then the exit code is 0
    And the output contains "credential: SOME_TOKEN -> api.some-provider.example"

  Scenario: a mixin the user names reads as the tag and the digest it pinned to
    Given the service inspects "registry.example.test/some-sandbox:1.0" as a sandbox the user's mixin "obs-tools:2" resolved into "ghcr.io/acme/obs@sha256:5b9e1f0a7c3d284e6b15f907a2c8d63b40e19a7c25f8b0d3e6a94c17f582aa41"
    When the user runs "lns inspect registry.example.test/some-sandbox:1.0 --mixin obs-tools:2"
    Then the exit code is 0
    And the output contains "mixin: obs-tools:2 → ghcr.io/acme/obs@sha256:5b9e1f0a7c3d284e6b15f907a2c8d63b40e19a7c25f8b0d3e6a94c17f582aa41"

  Scenario: inspecting a published sandbox names the mixins it resolved into
    Given the service inspects "registry.example.test/some-sandbox:1.0" as a sandbox resolved from the mixin "ghcr.io/acme/postgres-tools@sha256:c41e8b7d20a95f6c3d84b1e07f92a5c8d63b40e19a7c25f8b0d3e6a94c17f582"
    When the user runs "lns inspect registry.example.test/some-sandbox:1.0"
    Then the exit code is 0
    And the output contains "mixin: ghcr.io/acme/postgres-tools@sha256:c41e8b7d20a95f6c3d84b1e07f92a5c8d63b40e19a7c25f8b0d3e6a94c17f582"

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
