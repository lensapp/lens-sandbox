Feature: a declared credential left to an installed connector

  A sandbox declares the credential variables its workload reads. Where a
  declaration has no bound value and an installed connector claims the same
  variable, the declaration is left to that connector: the workload boots
  holding that connector's marker, and the consent card settles it.

  Installing a connector on its own settles nothing. A project that never
  declared the variable never sees it, so a tool that switches a feature on
  because a variable exists stays off.

  Scenario: a declared variable an installed connector claims boots holding its marker
    Given the sandbox declares the credential SOME_TOKEN
    And the installed connector "some-provider" claims SOME_TOKEN with the placeholder some_LNSPLACEHOLDER0000000000
    When the user runs `lns run someimage`
    Then the workload's environment contains SOME_TOKEN set to "some_LNSPLACEHOLDER0000000000"
    And the run is told nothing about SOME_TOKEN

  Scenario: a variable no sandbox declared is never set by an install alone
    Given the installed connector "some-provider" claims SOME_INTEGRATION_TOKEN with the placeholder some_LNSPLACEHOLDER0000000000
    When the user runs `lns run someimage`
    Then the workload's environment carries no SOME_INTEGRATION_TOKEN entry

  Scenario: a value the user set outranks a marker left to a connector
    Given the sandbox declares the credential SOME_TOKEN
    And the installed connector "some-provider" claims SOME_TOKEN with the placeholder some_LNSPLACEHOLDER0000000000
    When the user runs `lns run -e SOME_TOKEN=mine someimage`
    Then the workload's environment contains SOME_TOKEN set to "mine"
    And the run is told nothing about SOME_TOKEN

  Scenario: a grant still displaces what the user set
    Given the sandbox declares the credential SOME_TOKEN
    And the connector "some-provider" fills SOME_TOKEN with the placeholder some_LNSPLACEHOLDER0000000000 for this run
    When the user runs `lns run -e SOME_TOKEN=sk-live-real someimage`
    Then the workload's environment contains SOME_TOKEN set to "some_LNSPLACEHOLDER0000000000"
    And the run is told "some-provider" fills SOME_TOKEN
