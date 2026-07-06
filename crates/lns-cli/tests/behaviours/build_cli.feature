@todo
Feature: building a typed artifact from the CLI
  `lns build` is the producer entry point: it packages a manifest or a
  directory into a typed artifact, optionally pushing and signing it. The
  positional PATH is a manifest file (kind inferred from its `kind:`) or a
  directory (packaged as a FileSet, which then requires --mount). --check
  validates without producing anything. A real secret is rejected outright;
  an over-broad shipped policy is only warned about, not rejected. The
  secret-guard and schema-validation mechanisms are pinned by Layer 3 units;
  this harness pins the argv shapes and how the CLI surfaces the outcome.

  Scenario: the build command is discoverable in help
    When I run "lns build --help"
    Then the exit code is 0
    And the output contains "Usage: lns build"
    And the output contains "--mount"
    And the output contains "--push"
    And the output contains "--sign"
    And the output contains "--check"

  Scenario: packaging a directory requires a mount path
    Given the service refuses the build with "a directory PATH requires --mount"
    When the user runs build command "./skills/deep-research -t some-registry.example/skills/deep:1"
    Then the exit code is 1
    And the output contains "--mount"

  Scenario: a directory packaged with a mount path builds a FileSet
    Given the service builds "./skills/deep-research" as a FileSet mounted at "/root/.some-agent/skills/deep-research"
    When the user runs build command "./skills/deep-research --mount /root/.some-agent/skills/deep-research -t some-registry.example/skills/deep:1"
    Then the exit code is 0
    And the output contains "FileSet"

  Scenario: --check validates without building or pushing
    Given the service validates the manifest "bundle.yaml" as sound
    When the user runs build command "bundle.yaml --check"
    Then the exit code is 0
    And the output contains "OK"
    And no artifact is pushed

  Scenario: a manifest carrying a real secret is refused
    Given the service refuses the build with "manifest carries a real secret; use a self-identifying placeholder"
    When the user runs build command "bundle.yaml"
    Then the exit code is 1
    And the output contains "real secret"

  Scenario: a shipped policy that defaults to allow builds with a prominent warning
    Given the service builds "bundle.yaml" but warns "shipped policy uses defaultVerdict: allow"
    When the user runs build command "bundle.yaml -t some-registry.example/some-agent:variant"
    Then the exit code is 0
    And the output contains "defaultVerdict: allow"

  Scenario: a shipped policy with a broad wildcard or CIDR allow builds with a warning
    Given the service builds "bundle.yaml" but warns "shipped policy has a broad allow: *"
    When the user runs build command "bundle.yaml -t some-registry.example/some-agent:variant"
    Then the exit code is 0
    And the output contains "broad allow"

  Scenario: overlapping filesets within a bundle build with a warning
    Given the service builds "bundle.yaml" but warns "filesets overlap at /root/.some-agent/settings.json"
    When the user runs build command "bundle.yaml -t some-registry.example/some-agent:variant"
    Then the exit code is 0
    And the output contains "overlap"

  Scenario: building a bundle reports the digests it pinned its components to
    Given the service builds "bundle.yaml" pinning its fileset to digest "sha256:abcd"
    When the user runs build command "bundle.yaml -t some-registry.example/some-agent:research"
    Then the exit code is 0
    And the output contains "sha256:abcd"

  Scenario: a component left on a floating tag is flagged at build
    Given the service refuses the build with "component left on a floating tag; pin it to a digest"
    When the user runs build command "bundle.yaml -t some-registry.example/some-agent:research"
    Then the exit code is 1
    And the output contains "floating tag"