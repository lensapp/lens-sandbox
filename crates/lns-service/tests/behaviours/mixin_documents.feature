Feature: a document may declare the mixins it layers on
  A sandbox states the mixins it builds on under `spec.mixins`, and a mixin may
  build on others the same way. Resolution happens at startup — each mixin is
  pulled and merged before the run presents the resolved sandbox for approval —
  and that is not implemented yet. So a run refuses, naming what it could not
  resolve, rather than booting a sandbox without the capabilities its own
  document declares. Authoring and publishing already work: the document
  validates offline, and `lns push` publishes a digest-pinned list as written.

  Scenario: a declared mixin refuses the launch until startup resolution lands
    Given the sandbox definition declares the mixin "ghcr.io/acme/postgres-tools@sha256:c41e8b7d20a95f6c3d84b1e07f92a5c8d63b40e19a7c25f8b0d3e6a94c17f582"
    When the sandbox is launched
    Then the launch is refused
    And the error names "ghcr.io/acme/postgres-tools"
    And the error says startup resolution is not implemented

  Scenario: a pulled sandbox's declared mixin refuses the launch too
    Given the sandbox definition declares the mixin "ghcr.io/acme/postgres-tools@sha256:c41e8b7d20a95f6c3d84b1e07f92a5c8d63b40e19a7c25f8b0d3e6a94c17f582"
    When the published sandbox is launched
    Then the launch is refused
    And the error says startup resolution is not implemented
