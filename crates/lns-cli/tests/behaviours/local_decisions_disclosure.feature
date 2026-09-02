Feature: the disclosure names the source that decided each rule
  A mixin the user layers on resolves after the sandbox that declares it, so the
  summary printed before boot lists its rules the way it lists every other
  source's: each entry named by the source that decided it, the later one ahead
  of what it overruled. An override nobody intended is visible while the run can
  still be refused.

  Scenario: a rule a layered mixin decided is listed with the source that decided it
    Given the sandbox denies "docs.some-vendor.example" and a layered mixin allows it
    When the run summary is composed before boot
    Then the run summary attributes "allow docs.some-vendor.example" to "team-egress.yaml"
    And the run summary attributes "deny docs.some-vendor.example" to "the sandbox"

  Scenario: the layered mixin is named among the sources the run resolved
    Given the sandbox denies "docs.some-vendor.example" and a layered mixin allows it
    When the run summary is composed before boot
    Then the run summary lists "Mixins:    team-egress.yaml"

  Scenario: a run that layered nothing has no second author to name
    Given the sandbox denies "docs.some-vendor.example" and nothing is layered on it
    When the run summary is composed before boot
    Then the run summary lists "Rules:     deny docs.some-vendor.example"
    And the run summary does not contain "[from"

  Scenario: a rule that says why it is in the file says so before boot too
    Given the sandbox denies "docs.some-vendor.example" and a layered mixin allowed it during a run
    When the run summary is composed before boot
    Then the run summary lists "allow docs.some-vendor.example  [from team-egress.yaml]  approved during a run"

  Scenario: a rule the sandbox explains says so even where there is no source to name
    Given the sandbox denies "docs.some-vendor.example" with a note and nothing is layered on it
    When the run summary is composed before boot
    Then the run summary lists "deny docs.some-vendor.example  the vendor mirrors the API here"
    And the run summary does not contain "[from"

  Scenario: the disclosure says where an answer given at a prompt will go
    Given the sandbox denies "docs.some-vendor.example" and nothing is layered on it
    When the run summary is composed before boot
    Then the run summary lists "Decisions: recorded in this run, and removed with it"
