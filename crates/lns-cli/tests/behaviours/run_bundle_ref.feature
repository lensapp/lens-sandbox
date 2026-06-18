Feature: Running a bundle reference

  `lns run <bundle-ref>` resolves the bundle's single agent and materializes the
  bundle's egress policy into a run-scoped ephemeral file, leaving the project's
  own `lns-policy.yaml` untouched.

  Scenario: a bundle resolves its agent and applies its egress policy ephemerally
    Given a bundle "localhost:5000/org/acme/bundles/some-system:v1" with agent image "localhost:5000/org/acme/images/some-agent:v1" and an egress policy
    When the developer launches "localhost:5000/org/acme/bundles/some-system:v1"
    Then the resolved image is "localhost:5000/org/acme/images/some-agent:v1"
    And the run uses an ephemeral policy outside the project directory
    And no policy file is created in the project directory
