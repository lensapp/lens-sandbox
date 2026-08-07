Feature: the run-as user a definition asks for
  A sandbox that needs a particular user — root to install packages, or a
  service account the image expects — can say so in `spec.user`, so the
  definition is runnable as published instead of needing a wrapper to pass
  `-u`. The per-run flag still wins, and the image's own `USER` is still
  the fallback when the definition says nothing.

  Scenario: A definition can ask for root without a wrapper flag
    Given the definition declares user "root"
    And the image declares no USER
    When the run-as user is resolved for `lns run .`
    Then the workload runs as "root"
    And the workload's uid is left for the guest to resolve

  Scenario: A definition may name a uid directly
    Given the definition declares user "1000"
    When the run-as user is resolved for `lns run .`
    Then the workload runs as "1000" with uid 1000

  Scenario: The definition's user wins over the image USER
    Given the definition declares user "node"
    And the image declares USER "www-data"
    When the run-as user is resolved for `lns run .`
    Then the workload runs as "node"

  Scenario: -u still outranks the definition
    Given the definition declares user "node"
    When the run-as user is resolved for `lns run -u root .`
    Then the workload runs as "root"

  Scenario: The image USER still applies when the definition is silent
    Given the image declares USER "www-data"
    When the run-as user is resolved for `lns run .`
    Then the workload runs as "www-data"

  Scenario: A definition splits USER:GROUP the way the flag does
    Given the definition declares user "node:staff"
    When the run-as user is resolved for `lns run .`
    Then the workload runs as "node" in group "staff"

  Scenario: A named user the definition asks for is resolved in the guest, not guessed
    Given the definition declares user "node"
    When the run-as user is resolved for `lns run .`
    Then the workload's uid is left for the guest to resolve

  Scenario: Neither a definition nor an image leaves the unprivileged default
    When the run-as user is resolved for `lns run .`
    Then the workload runs as "sandbox" with uid 65534
