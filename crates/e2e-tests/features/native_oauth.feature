Feature: Native OAuth service wiring
  Scenario: Cancel a service-owned authorization without saving credentials
    Given a clean lns cache home
    And the LNS service is running headless in that home
    And a native device connector using an isolated loopback provider
    When I start native OAuth over the service socket
    And I choose the native read-only permission preset
    Then repeated native OAuth status requests share one background operation
    When I cancel native OAuth over the service socket
    Then native OAuth reports cancellation and keeps no connection

  Scenario: Native loopback callback adapter binds and releases the configured listener
    When the native loopback callback adapter receives a validated authorization response
    Then the callback is consumed once and its completion page contains no authorization values
