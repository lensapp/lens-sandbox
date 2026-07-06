@todo
Feature: --with overrides a bundle's mounted components at launch
  A consumer can swap one component version at launch without rebuilding
  the whole bundle — the "test a newer skill" lever. Each --with adds or
  replaces a mounted component, inferred by kind and name from the pulled
  artifact, and lands last in the overlay: on top of the base image and
  every bundle-declared fileset. --with is repeatable.

  Scenario: A --with fileset lands on top of the bundle's own filesets
    Given a bundle declaring a fileset mounting "shipped" at "/root/.some-agent/settings.json"
    When the bundle is run with --with a fileset mounting "override" at "/root/.some-agent/settings.json"
    Then the assembled workload has "/root/.some-agent/settings.json" content "override"

  Scenario: A --with fileset for a new path is added alongside the bundle's filesets
    Given a bundle declaring a fileset mounting "settings" at "/root/.some-agent/settings.json"
    When the bundle is run with --with a fileset mounting "extra" at "/root/.some-agent/skills/extra.md"
    Then the assembled workload has "/root/.some-agent/settings.json" from the bundle
    And the assembled workload has "/root/.some-agent/skills/extra.md" from the override

  Scenario: Multiple --with overrides all apply in the order given
    Given a bundle declaring a fileset mounting "shipped" at "/root/.some-agent/settings.json"
    When the bundle is run with --with mounting "first" at "/a" and --with mounting "second" at "/b"
    Then the assembled workload has "/a" from the first override
    And the assembled workload has "/b" from the second override

  Scenario: A --with override is refused when its kind is unsupported
    Given a bundle
    When the bundle is run with --with an artifact of kind "Workflow"
    Then the run is refused because the override kind is unsupported
    And nothing is launched