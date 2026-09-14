Feature: Reuse a service's MCP authentication for direct API access

  Scenario: Linear's browser sign-in produces a credential for both interfaces
    Given Linear authorizes its direct API and MCP with the same token
    When I finish the Linear connector's browser sign-in
    Then the Linear connection contains renewable credentials
    And both Linear interfaces were checked with the same token

  Scenario: A token that only authorizes MCP cannot become general API access
    Given Linear's MCP token is refused by its direct API
    When I finish the Linear connector's browser sign-in
    Then the Linear connector refuses to save general API access

  Scenario: An existing connection renews without opening a browser
    Given Linear authorizes its direct API and MCP with the same token
    When the Linear connector renews its credential
    Then the Linear connection contains renewable credentials
    And no browser was opened for renewal
