Feature: --with overrides a bundle's mounted components at launch
  A consumer can swap one component version at launch without rebuilding the
  whole bundle — the "test a newer skill" lever. Each --with adds or replaces
  a mounted component, inferred by kind and name from the pulled artifact, and
  lands last in the overlay: on top of the base image and every bundle-declared
  fileset. --with is repeatable, and only mountable (FileSet) kinds may override.

  Scenario: A --with fileset overrides a bundle fileset at the same path
    Given a bundle declaring a fileset "shipped" mounting "/root/.some-agent/settings.json"
    When the bundle is run with --with a fileset "override" mounting "/root/.some-agent/settings.json"
    Then "/root/.some-agent/settings.json" in the assembled workload comes from fileset "override"

  Scenario: A --with fileset at a new path is added alongside the bundle's filesets
    Given a bundle declaring a fileset "settings" mounting "/root/.some-agent/settings.json"
    When the bundle is run with --with a fileset "extra" mounting "/root/.some-agent/skills/extra.md"
    Then "/root/.some-agent/settings.json" in the assembled workload comes from fileset "settings"
    And "/root/.some-agent/skills/extra.md" in the assembled workload comes from fileset "extra"

  Scenario: A --with fileset overrides a path the base image ships
    Given a bundle whose sandbox base image ships "/root/.some-agent/settings.json"
    When the bundle is run with --with a fileset "override" mounting "/root/.some-agent/settings.json"
    Then "/root/.some-agent/settings.json" in the assembled workload comes from fileset "override"

  Scenario: Multiple --with overrides are all applied, the last winning a collision
    Given a bundle declaring a fileset "shipped" mounting "/x"
    When the bundle is run with --with a fileset "first" mounting "/y" and --with a fileset "second" mounting "/x"
    Then "/y" in the assembled workload comes from fileset "first"
    And "/x" in the assembled workload comes from fileset "second"

  Scenario: A --with override of an unsupported kind is refused
    Given a bundle declaring a fileset "shipped" mounting "/x"
    When the bundle is run with --with an artifact of kind "Workflow"
    Then the run is refused because the override kind is unsupported
    And nothing is assembled

  Scenario: A valid override paired with an unsupported one assembles nothing
    Given a bundle declaring a fileset "shipped" mounting "/x"
    When the bundle is run with --with a fileset "good" mounting "/y" and --with an artifact of kind "Workflow"
    Then the run is refused because the override kind is unsupported
    And nothing is assembled

  Scenario: A --with fileset override with no mount path is refused
    Given a bundle declaring a fileset "shipped" mounting "/x"
    When the bundle is run with --with a fileset "override" carrying no mount path
    Then the run is refused because the override has no mount path
    And nothing is assembled

  Scenario: A --with fileset override with a traversing mount path is refused
    Given a bundle declaring a fileset "shipped" mounting "/x"
    When the bundle is run with --with a fileset "escape" mounting "/root/../../etc"
    Then the run is refused because the override mount path is unsafe
    And nothing is assembled
