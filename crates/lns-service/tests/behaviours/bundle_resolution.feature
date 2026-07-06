Feature: bundle resolution is atomic and fail-closed
  The full component graph — the bundle, its sandbox and base image, its
  agent, and its filesets — is resolved and verified up front. Any failure
  aborts the whole run with a clear, component-named error before the
  workload starts, so a half-resolved bundle can never launch.

  Scenario: A well-formed bundle resolves into a composed workload
    Given a bundle whose sandbox, agent, and fileset are all present and supported
    When the bundle is resolved
    Then resolution succeeds
    And every declared component was fetched
    And the resolved bundle's base image is "registry.example.test/base:1"
    And the resolved bundle runs command "agent --serve"
    And the resolved bundle includes fileset "skills"

  Scenario: A component shared by two parents is resolved only once
    Given a bundle whose sandbox and agent both reference the same fileset
    When the bundle is resolved
    Then resolution succeeds
    And the shared fileset was fetched exactly once

  Scenario: A bundle with no sandbox is refused
    Given a bundle with no sandbox
    When the bundle is resolved
    Then resolution is refused
    And the refusal mentions "exactly one sandbox"
    And the refusal mentions "found none"

  Scenario: A bundle with more than one sandbox is refused
    Given a bundle with two sandboxes
    When the bundle is resolved
    Then resolution is refused
    And the refusal mentions "exactly one sandbox"
    And the refusal mentions "found more than one"

  Scenario: A bundle with no agent is refused
    Given a bundle with no agent
    When the bundle is resolved
    Then resolution is refused
    And the refusal mentions "exactly one agent"
    And the refusal mentions "found none"

  Scenario: A bundle with more than one agent is refused
    Given a bundle with two agents
    When the bundle is resolved
    Then resolution is refused
    And the refusal mentions "exactly one agent"
    And the refusal mentions "found more than one"

  Scenario: A missing component aborts the run and names it
    Given a bundle referencing a fileset "skills" that is not present in the registry
    When the bundle is resolved
    Then resolution is refused
    And the refusal names the missing component "skills"

  Scenario: A component of an unsupported kind aborts the run
    Given a bundle referencing a component of kind "Workflow"
    When the bundle is resolved
    Then resolution is refused
    And the refusal names the unsupported kind "Workflow"

  Scenario: A malformed component aborts the run
    Given a bundle referencing a component that is malformed
    When the bundle is resolved
    Then resolution is refused
    And the refusal mentions "malformed"

  Scenario: A base image built for another architecture aborts the run
    Given a bundle whose sandbox base image is built for a foreign architecture
    When the bundle is resolved
    Then resolution is refused
    And the refusal reports both the image and host architectures

  Scenario: A reference cycle in the component graph aborts the run
    Given a bundle whose component graph contains a reference cycle
    When the bundle is resolved
    Then resolution is refused
    And the refusal reports the reference cycle

  Scenario: Two components sharing a name abort the run
    Given a bundle declaring two components both named "skills"
    When the bundle is resolved
    Then resolution is refused
    And the refusal names the duplicated component "skills"

  Scenario: A nested bundle aborts the run
    Given a bundle referencing another bundle as a component
    When the bundle is resolved
    Then resolution is refused
    And the refusal reports that nested bundles are not allowed

  Scenario: A component needing a login on another registry aborts the run and names the host
    Given a bundle whose fileset lives on "other-registry.example.test" with no stored credential
    When the bundle is resolved
    Then resolution is refused
    And the refusal names the registry host "other-registry.example.test" that needs a login
