Feature: Native public-client OAuth
  Standard OAuth sign-in keeps real credentials on the host and does not need a component.

  Scenario Outline: A public client signs in with structured progress
    Given a public client using native "<flow>" OAuth
    Then the native connector installs without a component or an account connection
    When the user starts native authorization
    Then native authorization has made no provider request yet
    When the service advances native authorization
    Then LNS presents structured authorization progress
    When the provider authorizes the native OAuth operation
    And the service advances native authorization
    Then native authorization supplies one public access token and private renewal state

    Examples:
      | flow               |
      | device             |
      | authorization code |
