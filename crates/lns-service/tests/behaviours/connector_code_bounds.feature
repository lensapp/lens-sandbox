@todo
Feature: lns bounds a component it cannot read
  A declarative connector can be read before it is trusted. A component cannot, so
  what stands in for reading it is the bound lns enforces around it: the hosts it
  may reach, the capabilities its method declared, the deadline it runs under, and
  the schedule lns keeps for itself.

  Each bound is enforced by the host on every call and never trusted from the
  component. A refused call is handed back to the component as an error, so
  whether the connect then fails is the component's own decision — these scenarios
  say which component they use.

  Scenario: a component may reach only the hosts its method declares
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method declaring the host "auth.some-provider.example"
    And its component fails when a call is refused
    When its component calls "other.some-provider.example" while connecting
    Then the call is refused
    And the connect fails
    And "sign-in" is still offered

  Scenario: a component may use a capability only when its method declares it
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method declaring no host execution
    And its component fails when a call is refused
    When its component tries to run a host program
    Then the attempt is refused
    And the connect fails

  Scenario: lns owns the schedule, and a renewal cannot shorten it
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method
    And the run "1a2b3c4d" grants "sign-in" through that connection
    When its component returns a renewal reporting an expiry one second away
    Then the next refresh is no sooner than the floor lns sets

  Scenario: a component past its deadline is stopped, and the offer stands
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method
    And its component never returns
    When the machine connects "sign-in"
    Then the component is stopped at its deadline
    And the connect fails
    And "sign-in" is still offered

  Scenario: the workload cannot cause a component to run
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method
    And the run "1a2b3c4d" grants "sign-in" through that connection
    When the workload requests "api.some-provider.example"
    Then no connect runs
    And no revoke runs
