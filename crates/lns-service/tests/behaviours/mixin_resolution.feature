Feature: a run resolves the mixins its document declares
  A document states the mixins it layers on, and the run resolves them before it
  plans anything: each is pulled, its own mixins after it, and the whole list
  merges into the one document that boots. What a mixin contributes is enforced
  exactly like something the sandbox wrote itself.

  Resolution needs the network the first time. A digest-pinned reference pulled
  once resolves from the manifest cache afterwards, because the digest is the
  whole identity.

  Resolution is what empties a document's mixin list, so one that still carries
  an entry never went through it and is refused rather than booted without what
  it contributes.

  The run's own decisions are the last source in that list, so they merge
  into the document like any other and the disclosure attributes them to the
  file. What the guest enforces is folded from that file live, so a rule the
  developer deletes mid-run stops applying.

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

  Scenario: what the run decided reaches the run, after everything it pulled
    Given a mixin declaring the tool "python@3.11"
    And the sandbox definition declares that mixin
    And the run's own decisions declare the tool "python@3.12"
    When the published sandbox is resolved and launched
    Then the run installs "python@3.12"

  Scenario: what the run decided merges into the document that boots
    Given the sandbox definition declares nothing but its image
    And the run's own decisions allow "api.some-provider.example"
    When the published sandbox is resolved and launched
    Then the run installs "curl@8"
    And the resolved document allows "api.some-provider.example"
    And the disclosure attributes "allow api.some-provider.example" to "decisions.yaml"

  Scenario: a run that decided only destinations is a source the disclosure names
    Given the sandbox definition declares nothing but its image
    And the run's own decisions allow "api.some-provider.example" and nothing else
    When the published sandbox is resolved and launched
    Then the run resolved its own decisions as a source

  Scenario: what the run decided outranks a destination the sandbox denies
    Given the sandbox definition denies "docs.some-vendor.example"
    And the run's own decisions allow "docs.some-vendor.example" and nothing else
    When the published sandbox is resolved and launched
    Then a workload request to "docs.some-vendor.example" is allowed by policy
    And the disclosure attributes "allow docs.some-vendor.example" to "decisions.yaml"
    And the disclosure attributes "deny docs.some-vendor.example" to "the sandbox"

  Scenario: a destination the developer never decided stays the sandbox's to deny
    Given the sandbox definition denies "docs.some-vendor.example"
    And the run's own decisions allow "api.some-provider.example" and nothing else
    When the published sandbox is resolved and launched
    Then a workload request to "docs.some-vendor.example" is denied by policy

  Scenario: a mixin the run's decisions name merges before them
    Given a mixin declaring the tools "node@22" and "python@3.11"
    And the sandbox definition declares nothing but its image
    And the run's own decisions declare the tool "python@3.12" and that mixin
    When the published sandbox is resolved and launched
    Then the run installs "node@22"
    And the run installs "python@3.12"

  Scenario: a mixin's egress is enforced like the sandbox's own
    Given a mixin allowing "api.some-provider.example"
    And the sandbox definition declares that mixin
    When the published sandbox is resolved and launched
    Then a workload request to "api.some-provider.example" is allowed by policy

  Scenario: a mixin cannot open a run that denies by default
    Given a mixin allowing "api.some-provider.example"
    And the sandbox definition declares that mixin
    And the run's decisions.yaml denies all by default
    When the published sandbox is resolved and launched
    Then a workload request to "api.some-provider.example" is denied by policy

  Scenario: a mixin that cannot be pulled refuses the run
    Given the sandbox definition declares a mixin nothing can resolve
    When the published sandbox is resolved and launched
    Then the launch is refused
    And the error says the mixin could not be resolved

  Scenario: a document that reached the plan unresolved refuses the run
    Given a mixin declaring the tool "python@3.12"
    And the sandbox definition declares that mixin
    When the sandbox is launched
    Then the launch is refused
    And the error says the definition reached the plan unresolved
