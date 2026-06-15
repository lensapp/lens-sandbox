Feature: authenticating to an OCI registry

  Scenario: Logging in stores a credential locally
    When the developer logs in to "registry.example.test" with username "any" and token "lns_secret_token"
    Then a credential for "registry.example.test" is stored

  Scenario: Listing credentials shows the registry and username but never the token
    Given a stored credential for "registry.example.test" with username "ci" and token "lns_secret_token"
    When the developer lists stored credentials
    Then the output contains "registry.example.test"
    And the output contains "ci"
    And the output does not contain "lns_secret_token"

  Scenario: Logging out removes the stored credential
    Given a stored credential for "registry.example.test" with username "any" and token "lns_secret_token"
    When the developer logs out of "registry.example.test"
    Then no credential for "registry.example.test" is stored

  Scenario: Login refuses a token unless it is piped via --password-stdin
    When the developer logs in to "registry.example.test" without --password-stdin
    Then the command fails with an exit code other than 0
