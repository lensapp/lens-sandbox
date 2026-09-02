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

  Scenario: a bare reference addresses the LNS hub rather than Docker Hub
    Given the service installs "docs" serving "docs.rs"
    When the user runs connector command "install acme/docs"
    Then the service was asked to install "hub.lns.run/acme/docs"

  Scenario: a method that authenticates is marked as needing a connect
    Given the service installs "some-provider" serving "api.some-provider.example"
    When the user runs connector command "install ghcr.io/acme/some-provider:1"
    Then the output says the method "API token" needs a connect

  Scenario: an install the service refuses reports why
    Given the connector service refuses with "other-provider already serves that destination"
    When the user runs connector command "install ghcr.io/acme/some-provider:1"
    Then the connector command fails
    And the connector error says "other-provider already serves that destination"

  Scenario: the list names what each connector serves and holds no connection yet
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    When the user runs connector command "list"
    Then the connector output names "some-provider"
    And the output says it holds no connection

  Scenario: a machine with nothing installed says so
    Given the service holds no connectors
    When the user runs connector command "list"
    Then the connector command succeeds
    And the output says no connectors are installed

  Scenario: uninstalling says the grants outlive it
    Given the service uninstalls "some-provider" dropping 2 connections
    When the user runs connector command "uninstall some-provider"
    Then the connector command succeeds
    And the output says runs keep what they granted
    And the output says 2 connections were dropped

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

  Scenario: connecting asks for the value the authentication produces, and names it
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And the service connects "some-provider" as "token"
    And the user types ""
    And the user types "sk-live-real"
    When the user runs connector command "connect some-provider --method token"
    Then the connector command succeeds
    And the prompt names the authentication "token"
    And the prompt says the value is not shown
    And the connector output does not contain "sk-live-real"
    And the output says connecting is not granting

  Scenario: connecting suggests a name the machine does not already hold
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And the machine holds the connection "token" of "some-provider" for method "token"
    And the service connects "some-provider" as "token-2"
    And the user types ""
    And the user types "sk-live-real"
    When the user runs connector command "connect some-provider --method token"
    Then the connector command succeeds
    And the prompt suggests the name "token-2"

  Scenario: connecting a method that does not authenticate is refused before a value is typed
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And the user types "sk-live-real"
    When the user runs connector command "connect some-provider --method open"
    Then the connector command fails
    And the connector error says "nothing to connect"
    And the connector output does not contain "sk-live-real"

  Scenario: connecting with no terminal refuses without naming a flag
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And there is no terminal
    When the user runs connector command "connect some-provider --method token"
    Then the connector command fails
    And the connector error says "No flag answers it"

  Scenario: an empty token connects nothing
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And the service connects "some-provider" as "token"
    And the user types ""
    When the user runs connector command "connect some-provider --method token"
    Then the connector command fails
    And the connector error says "nothing was connected"

  Scenario: granting discloses the whole payload before it asks
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And the service grants "some-provider" the method "token"
    And the user types "y"
    When the user runs connector command "grant some-provider --run reviewer --method token"
    Then the connector command succeeds
    And the disclosure names what the method opens
    And the disclosure names the file it writes
    And the disclosure names the variables it sets
    And the output says it was granted

  Scenario: a payload the method does not carry is stated, not omitted
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And the service grants "some-provider" the method "open"
    And the user types "y"
    When the user runs connector command "grant some-provider --run reviewer --method open"
    Then the connector command succeeds
    And the disclosure says the method carries nothing to disclose

  Scenario: a grant this run already holds exits 1 and says so
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And the service reports the grant unchanged for "some-provider"
    And the user types "y"
    When the user runs connector command "grant some-provider --run reviewer --method token"
    Then the connector command exits 1
    And the output says it was already granted

  Scenario: declining the disclosure grants nothing and exits 1
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And the service grants "some-provider" the method "token"
    And the user types "n"
    When the user runs connector command "grant some-provider --run reviewer --method token"
    Then the connector command exits 1
    And the output says nothing was granted

  Scenario: granting with no terminal refuses, and no flag answers it
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And there is no terminal
    When the user runs connector command "grant some-provider --run reviewer --method token"
    Then the connector command fails
    And the connector error says "granting"
    And the connector error says "No flag answers it"

  Scenario: granting with no --run is a usage error, because there is no directory to fall back on
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And the user types "y"
    When the user runs connector command "grant some-provider --method token"
    Then the connector command fails
    And the connector error says "--run"

  Scenario: the disclosure names the run being granted
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And the service grants "some-provider" the method "token"
    And the user types "y"
    When the user runs connector command "grant some-provider --run reviewer --method token"
    Then the connector command succeeds
    And the disclosure names the run "reviewer"

  Scenario: granting a name no run holds says so before it asks
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And no run answers to that name
    And the service grants "some-provider" the method "token"
    And the user types "y"
    When the user runs connector command "grant some-provider --run revieweer --method token"
    Then the connector command succeeds
    And the disclosure says no run is named "revieweer"
    And the output says it reserved the decision

  Scenario: forgetting clears a reservation and says which it cleared
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And no run answers to that name
    And the service forgets a decision about "some-provider"
    When the user runs connector command "forget some-provider --run revieweer"
    Then the connector command succeeds
    And the output says it forgot the reservation

  Scenario: granting again names the method it replaced
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And the service grants "some-provider" the method "token" replacing "session"
    And the user types "y"
    When the user runs connector command "grant some-provider --run reviewer --method token"
    Then the connector command succeeds
    And the output says it replaced "session"

  Scenario: disconnecting a connector holding no connection exits 1
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And the service disconnects "some-provider" dropping 0 connections
    When the user runs connector command "disconnect some-provider"
    Then the connector command exits 1
    And the output says it holds no connection to disconnect

  Scenario: disconnecting says the connector stays installed
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And the service disconnects "some-provider" dropping 2 connections
    When the user runs connector command "disconnect some-provider"
    Then the connector command succeeds
    And the output says it stays installed

  Scenario: forgetting a run that decided nothing exits 1
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And the service forgets nothing about "some-provider"
    When the user runs connector command "forget some-provider --run reviewer"
    Then the connector command exits 1

  Scenario: forgetting clears what the run decided
    Given the service holds the connector "some-provider" serving "api.some-provider.example"
    And the service forgets a decision about "some-provider"
    When the user runs connector command "forget some-provider --run reviewer"
    Then the connector command succeeds
    And the output says it forgot the decision
