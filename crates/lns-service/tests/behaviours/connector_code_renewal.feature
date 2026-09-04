@todo
Feature: renewing and dropping what a code method produced
  A credential a host tool owns expires on the provider's clock, not on a button
  press, and nothing bounds how long a run lives. So a run outliving its
  credential is the ordinary case rather than the edge one, and `refresh` is the
  one function lns calls unasked.

  What must survive a renewal is the rest of the connector's contract: a value
  reaches a running guest by arming its placeholder and never by any other route,
  a fileset still carries the placeholder rather than the value, identity the
  renewal did not restate is carried forward by lns, and a renewal that cannot be
  had raises the card instead of failing quietly against the provider.

  Scenario: a run outlives its credential and keeps working
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method
    And the run "1a2b3c4d" grants "sign-in" through that connection
    And the credential expires in five minutes
    When lns reaches that connection on its refresh schedule
    Then the run's placeholder is armed with the renewed value
    And the run is not restarted

  Scenario: a failing refresh raises the connect prompt on next use
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method
    And the run "1a2b3c4d" grants "sign-in" through that connection
    And every renewal attempt fails
    When the credential's expiry passes
    Then the run's placeholder is unarmed
    And the workload's next request to "api.some-provider.example" is held
    And the connect prompt asks the user to reconnect

  Scenario: lns carries scopes and account forward when a renewal omits them
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method
    And the machine connects "sign-in" reporting the account "jane@some-provider.example"
    And the connection holds the scopes it consented to
    When its component returns a renewal naming no scopes and no account
    Then the connection keeps the account it already had
    And the connection keeps the scopes it already had

  Scenario: a granted method's fileset carries the placeholder, never the value
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method writing a credentials file
    And the run "1a2b3c4d" grants "sign-in" through that connection
    When its component returns a renewal
    Then the file the guest holds carries the placeholder
    And the file the guest holds carries no renewed value

  Scenario: revoke drops the connection even when the component fails
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method
    And the machine holds a connection for "some-provider"
    And its component fails on revoke
    When the machine disconnects "some-provider"
    Then the machine holds no connection for "some-provider"
