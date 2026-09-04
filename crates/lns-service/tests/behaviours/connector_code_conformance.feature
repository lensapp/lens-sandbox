@todo
Feature: every mechanism behaves the same from outside
  `token`, and the OAuth kinds when they return, are mechanisms lns implements. A
  `code` method is the same interface with the author's implementation behind it.
  One interface, several adapters.

  Two adapter paths can drift, and the layering is worth nothing if they do. So
  these scenarios are written once and run against each mechanism in turn: what a
  connection records, what a failure leaves behind, and what a disconnect drops
  must not depend on which adapter produced the value. A mechanism that needs one
  of these to differ means the interface is wrong.

  Scenario Outline: a connection records the method that produced it
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" uses the <mechanism> mechanism
    When the machine connects "sign-in"
    Then the machine holds a connection for "some-provider"
    And the connection records the method "sign-in"

    Examples:
      | mechanism |
      | token     |
      | code      |

  Scenario Outline: a connect that fails leaves the offer standing
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" uses the <mechanism> mechanism
    And the mechanism fails
    When the machine connects "sign-in"
    Then the connect fails
    And the machine holds no connection for "some-provider"
    And "sign-in" is still offered

    Examples:
      | mechanism |
      | token     |
      | code      |

  Scenario Outline: a disconnect drops the connection
    Given the connector "some-provider" serves "api.some-provider.example"
    And its method "sign-in" uses the <mechanism> mechanism
    And the machine holds a connection for "some-provider"
    When the machine disconnects "some-provider"
    Then the machine holds no connection for "some-provider"

    Examples:
      | mechanism |
      | token     |
      | code      |
