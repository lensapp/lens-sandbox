Feature: run dispatches on the pulled artifact type
  `lns run <ref>` targets a sandbox: the pulled manifest's artifactType
  decides the path, falling back to the config-blob media type when a push
  tool (e.g. oras) leaves artifactType empty. A sandbox artifact assembles
  before launch; a plain OCI image is refused with a hint to `lns init`; any
  other typed artifact — or an unknown type — is refused rather than guessed at.

  Scenario: A plain OCI image reference is refused — it is not a sandbox
    Given a pulled reference whose manifest has no artifact type
    When the run resolves the reference for launch
    Then the run is refused because the reference is not a sandbox
    And the refusal points at "lns init"

  Scenario: A published sandbox artifact runs directly
    Given a pulled reference whose manifest is a sandbox artifact
    When the run resolves the reference for launch
    Then the run launches the sandbox directly

  Scenario: A bundle artifact takes the assembly path
    Given a pulled reference whose manifest is an "AgentSystem" bundle
    When the run resolves the reference for launch
    Then the run assembles the bundle before launching

  Scenario: A bundle identified only by its config media type takes the assembly path
    Given a pulled reference with no artifact type but a bundle config media type
    When the run resolves the reference for launch
    Then the run assembles the bundle before launching

  Scenario: An unknown artifact type is refused
    Given a pulled reference whose manifest artifact type is "application/vnd.unknown.thing"
    When the run resolves the reference for launch
    Then the run is refused because the artifact type is unsupported
    And the refusal names the unsupported type "application/vnd.unknown.thing"

  Scenario: A typed but non-runnable artifact is refused
    Given a pulled reference whose manifest is a fileset artifact
    When the run resolves the reference for launch
    Then the run is refused because the artifact is not directly runnable