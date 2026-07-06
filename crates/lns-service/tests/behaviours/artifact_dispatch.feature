Feature: run dispatches on the pulled artifact type
  `lns run <ref>` must keep behaving exactly as before for a plain OCI
  image, while a typed artifact takes the new assembly path. The pulled
  manifest's artifactType is the only signal that decides which path a
  run takes, so an unknown type must be refused rather than guessed at.

  Scenario: A plain OCI image takes the existing single-image path
    Given a pulled reference whose manifest has no artifact type
    When the run resolves the reference for launch
    Then the run launches the single image unchanged
    And no bundle assembly is performed

  Scenario: A bundle artifact takes the assembly path
    Given a pulled reference whose manifest is an "AgentSystem" bundle
    When the run resolves the reference for launch
    Then the run assembles the bundle before launching

  Scenario: An unknown artifact type is refused
    Given a pulled reference whose manifest artifact type is "application/vnd.unknown.thing"
    When the run resolves the reference for launch
    Then the run is refused because the artifact type is unsupported
    And the refusal names the unsupported type "application/vnd.unknown.thing"