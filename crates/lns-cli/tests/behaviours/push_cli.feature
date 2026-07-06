@todo
Feature: pushing a typed artifact from the CLI
  `lns push` uploads a locally-built artifact, reusing the credential
  stored by `lns login`. Registry login today verifies only pull scope, so
  a token lacking push scope is accepted at login and must fail clearly
  here. --sign attaches a cosign-compatible signature as an OCI referrer.

  Scenario: the push command is discoverable in help
    When I run "lns push --help"
    Then the exit code is 0
    And the output contains "Usage: lns push"
    And the output contains "--sign"

  Scenario: pushing an artifact reports the reference it landed at
    Given the service pushes "some-registry.example/some-agent:research" successfully
    When the user runs push command "some-registry.example/some-agent:research"
    Then the exit code is 0
    And the output contains "some-registry.example/some-agent:research"

  Scenario: a credential lacking push scope fails clearly and names the host
    Given the service refuses the push with "credential for some-registry.example lacks push scope"
    When the user runs push command "some-registry.example/some-agent:research"
    Then the exit code is 1
    And the output contains "lacks push scope"
    And the output contains "some-registry.example"

  Scenario: --sign attaches a signature to the pushed artifact
    Given the service pushes "some-registry.example/some-agent:research" and attaches a signature
    When the user runs push command "some-registry.example/some-agent:research --sign"
    Then the exit code is 0
    And the output contains "signed"