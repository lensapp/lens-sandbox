Feature: a bundle run is recorded in the audit chain
  Reproducing exactly what ran means recording exactly what was assembled.
  Each bundle run appends to the existing audit chain the bundle digest,
  every resolved component digest, any --with overrides, the effective
  policy source and a hash of it, the integration identities in effect,
  and the signature/trust verdict.

  Scenario: A bundle run records the bundle digest and every component digest
    Given a bundle assembled from a sandbox base image and two filesets
    When the run is recorded in the audit chain
    Then the audit record names the bundle digest
    And the audit record names the digest of every resolved component

  Scenario: A --with override is recorded in the audit chain
    Given a bundle run with a --with fileset override
    When the run is recorded in the audit chain
    Then the audit record names the --with override and its digest

  Scenario: The effective policy source and hash are recorded
    Given a bundle run governed by the bundle's shipped policy under a local overlay
    When the run is recorded in the audit chain
    Then the audit record names the effective policy source
    And the audit record carries a hash of the effective policy

  Scenario: The integration identities in effect are recorded
    Given a bundle whose agent uses integration "some-provider"
    When the run is recorded in the audit chain
    Then the audit record names the integration identity "some-provider" in effect

  Scenario: The signature and trust verdict is recorded
    Given a bundle signed by a trusted key
    When the run is recorded in the audit chain
    Then the audit record carries the signature and trust verdict