Feature: the disclosure names what this directory decided
  A run in a directory that has decided something resolves those decisions as the
  last source of its document, so the summary printed before boot lists their
  rules the way it lists every other source's: each entry named by the source that
  decided it, the developer's own ahead of what it overruled. An override nobody
  intended is visible while the run can still be refused.

  Scenario: a rule this directory decided is listed with the file that decided it
    Given the sandbox denies "docs.some-vendor.example" and this directory allows it
    When the run summary is composed before boot
    Then the run summary attributes "allow docs.some-vendor.example" to "lns-local-mixin.yaml"
    And the run summary attributes "deny docs.some-vendor.example" to "the sandbox"

  Scenario: the decisions file is named among the sources the run resolved
    Given the sandbox denies "docs.some-vendor.example" and this directory allows it
    When the run summary is composed before boot
    Then the run summary lists "Mixins:    lns-local-mixin.yaml"

  Scenario: a directory that decided nothing has no second author to name
    Given the sandbox denies "docs.some-vendor.example" and this directory decided nothing
    When the run summary is composed before boot
    Then the run summary lists "Rules:     deny docs.some-vendor.example"
    And the run summary does not contain "[from"

  Scenario: a rule that says why it is in the file says so before boot too
    Given the sandbox denies "docs.some-vendor.example" and this directory allowed it during a run
    When the run summary is composed before boot
    Then the run summary lists "allow docs.some-vendor.example  [from lns-local-mixin.yaml]  approved during a run"

  Scenario: a rule the sandbox explains says so even where there is no source to name
    Given the sandbox denies "docs.some-vendor.example" with a note and this directory decided nothing
    When the run summary is composed before boot
    Then the run summary lists "deny docs.some-vendor.example  the vendor mirrors the API here"
    And the run summary does not contain "[from"
