@todo
Feature: what a component did is recorded in the durable ledger
  A run's audit chain records what that run decided about a connector: granted,
  declined, forgotten. Connecting is deliberately not one of those, because a
  connection belongs to the machine and no run's chain could account for it. The
  durable ledger is where the machine's own history goes, and `lns audit` merges
  the two.

  A component makes that distinction load-bearing. Its outbound calls, the
  programs it starts, and every renewal lns schedules happen outside any run — and
  a renewal that ran while nobody watched is invisible unless it is written down.
  So each is a ledger entry, and the run chains keep meaning exactly what they
  meant before.

  Scenario: a component's outbound call is recorded in the durable ledger
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method declaring the host "auth.some-provider.example"
    When its component calls "auth.some-provider.example" while connecting
    Then the durable ledger records the call, naming "some-provider" and that host
    And no run's audit chain records it

  Scenario: a call refused for an undeclared host is recorded as refused
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method declaring the host "auth.some-provider.example"
    When its component calls "other.some-provider.example" while connecting
    Then the durable ledger records the call as refused

  Scenario: a program a component starts is recorded in the durable ledger
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method declaring host execution
    When its component runs a host program while connecting
    Then the durable ledger records the program it started

  Scenario: a renewal no run asked for is recorded in the durable ledger
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method
    And the run "1a2b3c4d" grants "sign-in" through that connection
    When lns reaches that connection on its refresh schedule
    Then the durable ledger records the renewal
    And the entry names the schedule rather than a user action

  Scenario: a run's audit chain still records only what that run decided
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method
    When the machine connects "sign-in"
    And the run "1a2b3c4d" grants "sign-in" through that connection
    Then the run's audit chain records the grant
    And the run's audit chain does not record the connect
