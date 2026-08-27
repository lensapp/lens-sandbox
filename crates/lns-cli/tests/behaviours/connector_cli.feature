Feature: lns connector, on this machine
  `lns connector` decides what this machine offers. Installing makes a connector
  offerable and grants nothing, so the output has to say that out loud — a user
  who installs one and expects a destination to open would otherwise find out
  much later.

  The service resolves what `<REF|PATH>` names, because only it can reach a
  registry. A path is made absolute before it is sent: the service's working
  directory is not the user's.

  Scenario: installing says what it did and what it did not do
    Given the service installs "some-provider" serving "api.some-provider.example"
    When the user runs connector command "install ghcr.io/acme/some-provider:1"
    Then the connector command succeeds
    And the output says nothing is granted yet
    And the output names the destination "api.some-provider.example"

  Scenario: a path operand reaches the service as an absolute path
    Given the service installs "some-provider" serving "api.some-provider.example"
    When the user runs connector command "install ./some-provider"
    Then the connector command succeeds
    And the service was asked to install an absolute path

  Scenario: a reference reaches the service unchanged
    Given the service installs "some-provider" serving "api.some-provider.example"
    When the user runs connector command "install ghcr.io/acme/some-provider:1"
    Then the service was asked to install "ghcr.io/acme/some-provider:1"

  Scenario: a method that authenticates is marked as needing a connect
    Given the service installs "some-provider" serving "api.some-provider.example"
    When the user runs connector command "install ghcr.io/acme/some-provider:1"
    Then the output says the method "API token" needs a connect

  Scenario: an install the service refuses reports why
    Given the connector service refuses with "other-provider already serves that destination"
    When the user runs connector command "install ghcr.io/acme/some-provider:1"
    Then the connector command fails
    And the connector error says "other-provider already serves that destination"

  Scenario: the list names what each connector serves and holds no profile yet
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    When the user runs connector command "list"
    Then the connector output names "some-provider"
    And the output says it holds no profile

  Scenario: a machine with nothing installed says so
    Given the service holds no connectors
    When the user runs connector command "list"
    Then the connector command succeeds
    And the output says no connectors are installed

  Scenario: uninstalling says the grants outlive it
    Given the service uninstalls "some-provider" dropping 2 profiles
    When the user runs connector command "uninstall some-provider"
    Then the connector command succeeds
    And the output says projects keep what they granted
    And the output says 2 profiles were dropped

  Scenario: uninstalling something that is not installed exits 1
    Given the service holds no connectors
    When the user runs connector command "uninstall absent"
    Then the connector command exits 1
    And the output says no connector named "absent" is installed

  Scenario: a service that does not answer is reported as such
    Given the connector service is unreachable
    When the user runs connector command "list"
    Then the connector command fails
    And the connector error mentions lns-service
