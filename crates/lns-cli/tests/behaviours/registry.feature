Feature: pushing and pulling artifacts to an OCI registry

  Scenario: A pushed policy can be pulled back with the same digest
    Given a policy file "lns-policy.yaml"
    When the developer pushes "lns-policy.yaml" to "localhost:5000/org/acme/policies/pii:v1"
    Then the push reports a sha256 digest
    When the developer pulls "localhost:5000/org/acme/policies/pii:v1" to "pulled.json"
    Then the pull reports the same digest as the push
    And the file "pulled.json" contains "defaultVerdict"

  Scenario: Pushing an agent infers the family from the reference path
    Given an agent file "agent.yaml"
    When the developer pushes "agent.yaml" to "localhost:5000/org/acme/agents/hermes:v1"
    Then the push reports a sha256 digest

  Scenario: Pushing with an explicit family override
    Given a policy file "p.yaml"
    When the developer pushes "p.yaml" to "localhost:5000/anything:v1" with family "policy"
    Then the push reports a sha256 digest

  Scenario: A non-file source is pushed as an image
    When the developer pushes "docker.io/library/alpine:3.20" to "localhost:5000/org/acme/images/alpine:3.20"
    Then the push reports a sha256 digest

  Scenario: Pushing a file whose family cannot be inferred fails
    Given a policy file "p.yaml"
    When the developer pushes "p.yaml" to "localhost:5000/just-a-name:v1"
    Then the command fails with an exit code other than 0

  Scenario: Pulling a reference that was never pushed fails
    When the developer pulls "localhost:5000/org/acme/agents/absent:v1" to "out.json"
    Then the command fails with an exit code other than 0
