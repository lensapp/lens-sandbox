Feature: --with overrides a bundle's mounted components at launch
  A consumer can swap one component version at launch without rebuilding the
  whole bundle — the "test a newer skill" lever. Each --with names a component
  by OCI reference; it resolves through the graph like any bundle component,
  taking its mount path from the referenced FileSet, and lands last in the
  overlay: on top of the base image and every bundle-declared fileset. --with
  is repeatable, and only mountable (FileSet) components may override.

  Scenario: A --with fileset overrides a bundle fileset at the same path
    Given a bundle declaring a fileset "shipped" mounting "/root/.some-agent/settings.json"
    When the bundle is run with --with a fileset "override" mounting "/root/.some-agent/settings.json"
    Then "/root/.some-agent/settings.json" in the assembled workload comes from fileset "override"

  Scenario: A --with fileset at a new path is added alongside the bundle's filesets
    Given a bundle declaring a fileset "settings" mounting "/root/.some-agent/settings.json"
    When the bundle is run with --with a fileset "extra" mounting "/root/.some-agent/skills/extra.md"
    Then "/root/.some-agent/settings.json" in the assembled workload comes from fileset "settings"
    And "/root/.some-agent/skills/extra.md" in the assembled workload comes from fileset "extra"

  Scenario: Multiple --with overrides are all applied, the last winning a collision
    Given a bundle declaring a fileset "shipped" mounting "/x"
    When the bundle is run with --with a fileset "first" mounting "/y" and --with a fileset "second" mounting "/x"
    Then "/y" in the assembled workload comes from fileset "first"
    And "/x" in the assembled workload comes from fileset "second"

  Scenario: A --with override that is not a mountable component is refused
    Given a bundle declaring a fileset "shipped" mounting "/x"
    When the bundle is run with --with an artifact of kind "Workflow"
    Then the run is refused because the override is not a mountable component
    And nothing is assembled
