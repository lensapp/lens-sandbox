Feature: pushing and pulling policies as OCI artifacts

  Scenario: A pushed policy can be pulled back unchanged
    Given "lns-policy.yaml" has an allow rule for "api.example.test"
    When the developer pushes "lns-policy.yaml" to "registry.example.test/org/acme/policies/pii:v1"
    And the developer pulls "registry.example.test/org/acme/policies/pii:v1" to "pulled.yaml"
    Then "pulled.yaml" contains an allow rule for "api.example.test"

  Scenario: The pull reports the same digest the push produced
    Given "lns-policy.yaml" has an allow rule for "api.example.test"
    When the developer pushes "lns-policy.yaml" to "registry.example.test/org/acme/policies/pii:v1"
    Then the push reports a sha256 digest
    When the developer pulls "registry.example.test/org/acme/policies/pii:v1" to "pulled.yaml"
    Then the pull reports the same digest as the push

  Scenario: Pulling a reference that was never pushed fails
    When the developer pulls "registry.example.test/org/acme/policies/absent:v1" to "out.yaml"
    Then the command fails with an exit code other than 0
