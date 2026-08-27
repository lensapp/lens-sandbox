Feature: installing a connector on this machine
  A connector is installed once per machine, and the installed set is what can be
  offered. Installing grants nothing: no destination opens, no variable is set,
  and no file is written. It only makes the connector's destinations ask, in every
  project that has neither granted nor declined it.

  Two installs must not leave an offer ambiguous, so the install refuses what a
  later launch could not decide: a `serves` that overlaps an installed
  connector's, and a variable an installed connector already claims.

  Scenario: an installed connector opens no destination and sets no variable
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "token" allows "api.some-provider.example" and sets "SOME_TOKEN"
    When the machine installs the connector
    Then the install succeeds
    And the machine holds the connector "some-provider"
    And the machine holds no profile for "some-provider"

  Scenario: a second connector serving a domain the first already covers refuses
    Given the connector "some-provider" serves "*.some-provider.example"
    And the machine installs the connector
    And the connector "other-provider" serves "api.some-provider.example"
    When the machine installs the connector
    Then the install is refused
    And the refusal says "some-provider" already serves that destination

  Scenario: a connector serving one port does not conflict with another port
    Given the connector "some-provider" serves "db.some-provider.example:5432"
    And the machine installs the connector
    And the connector "other-provider" serves "db.some-provider.example:6432"
    When the machine installs the connector
    Then the install succeeds

  Scenario: a method carrying a block a connector may not carry refuses
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "token" carries the block "tools"
    When the machine installs the connector
    Then the install is refused
    And the refusal names the block "tools"

  Scenario: a variable an installed connector already claims refuses
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "token" allows "api.some-provider.example" and sets "SHARED_TOKEN"
    And the machine installs the connector
    And the connector "other-provider" serves "api.other-provider.example"
    And its method "token" allows "api.other-provider.example" and sets "SHARED_TOKEN"
    When the machine installs the connector
    Then the install is refused
    And the refusal names the variable "SHARED_TOKEN"

  Scenario: two methods of one connector may claim one variable
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "token" allows "api.some-provider.example" and sets "SOME_TOKEN"
    And its method "session" allows "api.some-provider.example" and sets "SOME_TOKEN"
    When the machine installs the connector
    Then the install succeeds

  Scenario: uninstalling stops the offer and keeps what a project granted
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "token" allows "api.some-provider.example" and sets "SOME_TOKEN"
    And the machine installs the connector
    And the project "/work" granted the method "token"
    When the machine uninstalls the connector "some-provider"
    Then the machine holds no connector "some-provider"
    And the project "/work" still grants the method "token"

  Scenario: the list names what each connector serves
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "token" allows "api.some-provider.example" and sets "SOME_TOKEN"
    And the machine installs the connector
    When the machine lists its connectors
    Then the list names "some-provider" serving "api.some-provider.example"
    And the list marks the method "token" as needing a connect
