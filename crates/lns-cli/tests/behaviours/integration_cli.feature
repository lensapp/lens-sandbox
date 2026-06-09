Feature: connecting integrations from the CLI
  An integration is connected to a project from the CLI with
  `lns integration connect <id>`, which records it under `integrations:`
  in that directory's `lns-policy.yaml`. Most integrations authenticate
  with a credential value; an `oauth` integration authenticates by an
  interactive device sign-in the background service drives — so connecting
  one shows the verification URL and code and only records the integration
  once the sign-in completes. The live token set lands in the per-machine
  credential store, never in `lns-policy.yaml`. `lns integration list`
  shows, per integration, whether it authenticates by sign-in or a value.

  Scenario: Connecting an oauth integration signs in and then records it
    Given the background service is available to sign in
    When the developer runs "lns integration connect github_oauth"
    Then a verification URL and user code are shown
    And "github_oauth" is recorded under integrations in lns-policy.yaml
    And lns-policy.yaml carries no token material

  Scenario: Connecting an oauth integration fails clearly when the service is unavailable
    Given the background service is not available
    When the developer runs "lns integration connect github_oauth"
    Then the command fails noting the service is needed to sign in
    And "github_oauth" is not recorded in lns-policy.yaml

  Scenario: The catalog listing shows each integration's auth kind
    When the developer runs "lns integration list"
    Then "github_oauth" is listed as authenticating by oauth
    And "gitlab" is listed as authenticating by credential
