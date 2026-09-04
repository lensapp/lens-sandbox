@todo
Feature: a code method's component decides the connect flow
  A declarative kind tells lns what to ask for, so lns owns the flow. A `code`
  method does not: its component is a step machine lns drives, and every prompt,
  every wait, and the moment it completes are the component's decisions. lns
  renders what it is told to render.

  This is the whole of what separates `code` from a declarative kind. Everything
  else a connector does — what it serves, what it injects, where a value travels —
  stays declarative and is decided the same way for every kind.

  Scenario: the component decides what the card asks for
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method
    And its component asks for the fields "workspace id" and "api key"
    And its component marks "api key" secret and "workspace id" not
    When the machine connects "sign-in"
    Then the card asks for exactly those two fields, in that order
    And "api key" is marked secret and "workspace id" is not

  Scenario: the component decides to wait, and lns resumes it afterwards
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method
    And its component returns a wait before it completes
    When the machine connects "sign-in"
    Then the card shows what the component asked to display
    And lns resumes the component only once the wait it asked for has elapsed

  Scenario: the component completes with no question at all
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" is a code method
    And its component returns done from what it was handed
    When the machine connects "sign-in"
    Then the connection is made
    And no field is asked for
