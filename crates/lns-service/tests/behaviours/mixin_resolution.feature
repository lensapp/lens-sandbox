Feature: a run resolves the mixins its document declares
  A document states the mixins it layers on, and the run resolves them before it
  plans anything: each is pulled, its own mixins after it, and the whole list
  merges into the one document that boots. What a mixin contributes is enforced
  exactly like something the sandbox wrote itself.

  Resolution needs the network the first time. A digest-pinned reference pulled
  once resolves from the manifest cache afterwards, because the digest is the
  whole identity.

  A published sandbox resolves. A local document does not yet, so it is refused
  rather than booted without what its own mixins contribute.

  Scenario: what a mixin declares reaches the run
    Given a mixin declaring the tool "python@3.12"
    And the sandbox definition declares that mixin
    When the published sandbox is resolved and launched
    Then the run installs "python@3.12"

  Scenario: a mixin's version of a shared tool wins over the sandbox's own
    Given a mixin declaring the tool "node@22"
    And the sandbox definition declares the tool "node@20" and that mixin
    When the published sandbox is resolved and launched
    Then the run installs "node@22"

  Scenario: a mixin's egress is enforced like the sandbox's own
    Given a mixin allowing "api.some-provider.example"
    And the sandbox definition declares that mixin
    When the published sandbox is resolved and launched
    Then a workload request to "api.some-provider.example" is allowed by policy

  Scenario: a mixin cannot open a directory that denies by default
    Given a mixin allowing "api.some-provider.example"
    And the sandbox definition declares that mixin
    And the directory's lns-policy.yaml denies all by default
    When the published sandbox is resolved and launched
    Then a workload request to "api.some-provider.example" is denied by policy

  Scenario: a credential a mixin contributes is unarmed until a value is bound
    Given a mixin declaring the credential "MIXIN_TOKEN" for "api.some-provider.example"
    And the sandbox definition declares that mixin
    When the published sandbox is resolved and launched
    Then the workload's environment contains the placeholder under "MIXIN_TOKEN"
    And no value-decision prompt is shown before the workload starts

  Scenario: a mixin that cannot be pulled refuses the run
    Given the sandbox definition declares a mixin nothing can resolve
    When the published sandbox is resolved and launched
    Then the launch is refused
    And the error says the mixin could not be resolved

  Scenario: a local document's mixins refuse the run rather than being dropped
    Given a mixin declaring the tool "python@3.12"
    And the sandbox definition declares that mixin
    When the sandbox is launched
    Then the launch is refused
    And the error says a local document's mixins are not resolved yet
