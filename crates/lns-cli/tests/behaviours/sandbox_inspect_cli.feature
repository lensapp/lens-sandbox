Feature: inspecting a typed artifact before running it
  `lns inspect <ref>` is the type-aware, pre-run view of a cached sandbox: it
  names the kind, and for an AgentSystem bundle it lists what the bundle
  composes — the base image, the filesets with their mount paths, the policy,
  and the integrations — plus the signature and trust status, and it flags an
  over-broad shipped policy. It lets a consumer review the pieces before
  trusting a configured sandbox.

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

  Scenario: inspecting a sandbox shows its declared ports
    Given the service inspects "registry.example.test/some-sandbox:1.0" as a sandbox declaring ports 3003 and 8080:9090
    When the user runs "lns inspect registry.example.test/some-sandbox:1.0"
    Then the exit code is 0
    And the output contains "ports: 3003, 8080:9090"

  Scenario: inspecting a bundle lists what it composes
    Given the service inspects "some-registry.example/some-agent:research" as a bundle composing:
      | sandbox base | registry.example.test/base:1                    |
      | fileset      | settings -> /root/.some-agent/settings.json     |
      | fileset      | deep-research -> /root/.some-agent/skills/deep  |
      | integration  | some-provider                                   |
    When the user runs "lns inspect some-registry.example/some-agent:research"
    Then the exit code is 0
    And the output contains "AgentSystem"
    And the output contains "registry.example.test/base:1"
    And the output contains "/root/.some-agent/settings.json"
    And the output contains "some-provider"

  Scenario: inspecting a signed bundle reports the trust status
    Given the service inspects "some-registry.example/some-agent:research" as a bundle signed by a trusted key
    When the user runs "lns inspect some-registry.example/some-agent:research"
    Then the exit code is 0
    And the output contains "signed"
    And the output contains "trusted"

  Scenario: inspecting a bundle with a permissive defaultVerdict flags it
    Given the service inspects "some-registry.example/some-agent:research" as a bundle whose policy defaults to allow
    When the user runs "lns inspect some-registry.example/some-agent:research"
    Then the exit code is 0
    And the output contains "defaultVerdict: allow"

  Scenario: inspecting a bundle names the host that needs a login
    Given the service reports "inspect" needs a login for host "other-registry.example.test"
    When the user runs "lns inspect some-registry.example/some-agent:research"
    Then the exit code is 1
    And the output contains "other-registry.example.test"
