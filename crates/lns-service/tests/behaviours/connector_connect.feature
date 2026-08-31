Feature: connecting a connector and granting what it holds
  A connect stores the values an authentication returned; a grant arms the
  method's credentials from them. The value the connect is asked for and the
  value the grant looks up are the same value, so a credential that names which
  of its auth's outputs it draws on is armed like one that names none.

  Scenario: a credential naming its auth's output is armed by what the connect asked for
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "token" draws "SOME_TOKEN" from the auth output "token"
    And the machine installs the connector
    When the machine connects "token" with "sk-live"
    And the run "1a2b3c4d" grants "token" through that connection
    Then the run is supplied "Bearer sk-live" for "SOME_TOKEN"

  Scenario: a credential naming no output is armed by the auth's only one
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "token" draws "SOME_TOKEN" from no named output
    And the machine installs the connector
    When the machine connects "token" with "sk-live"
    And the run "1a2b3c4d" grants "token" through that connection
    Then the run is supplied "Bearer sk-live" for "SOME_TOKEN"
