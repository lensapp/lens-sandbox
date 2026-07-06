@todo
Feature: inspecting a typed artifact before running it
  `lns image inspect <ref>` is the type-aware, pre-run view: it names the
  kind, and for a bundle it lists what the bundle composes — the sandbox
  and base image, the filesets with their mount paths, the policy, and the
  integrations — plus the signature and trust status, and it flags an
  over-broad shipped policy. It lets a consumer review the pieces before
  trusting a configured agent.

  Scenario: inspect is discoverable in the image help
    When I run "lns image inspect --help"
    Then the exit code is 0
    And the output contains "Usage: lns image inspect"

  Scenario: inspecting a plain image labels it Image
    Given the service inspects "registry.example.test/some-image:1.0" as a plain image
    When the user runs image command "inspect some-image:1.0"
    Then the exit code is 0
    And the output contains "Image"

  Scenario: inspecting a bundle lists what it composes
    Given the service inspects "some-registry.example/some-agent:research" as a bundle composing:
      | sandbox base | registry.example.test/base:1                    |
      | fileset      | settings -> /root/.some-agent/settings.json     |
      | fileset      | deep-research -> /root/.some-agent/skills/deep  |
      | integration  | some-provider                                   |
    When the user runs image command "inspect some-agent:research"
    Then the exit code is 0
    And the output contains "AgentSystem"
    And the output contains "registry.example.test/base:1"
    And the output contains "/root/.some-agent/settings.json"
    And the output contains "some-provider"

  Scenario: inspecting a signed bundle reports the trust status
    Given the service inspects "some-registry.example/some-agent:research" as a bundle signed by a trusted key
    When the user runs image command "inspect some-agent:research"
    Then the exit code is 0
    And the output contains "signed"
    And the output contains "trusted"

  Scenario: inspecting a bundle with a permissive defaultVerdict flags it
    Given the service inspects "some-registry.example/some-agent:research" as a bundle whose policy defaults to allow
    When the user runs image command "inspect some-agent:research"
    Then the exit code is 0
    And the output contains "defaultVerdict: allow"

  Scenario: inspecting a bundle names the host that needs a login
    Given the service reports "inspect" needs a login for host "other-registry.example.test"
    When the user runs image command "inspect some-agent:research"
    Then the exit code is 1
    And the output contains "other-registry.example.test"