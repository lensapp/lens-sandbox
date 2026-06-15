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

  These scenarios use an arbitrary user-declared "some-oauth" integration, so
  nothing here pins a shipped service.

  Background:
    Given a user catalog declares the "some-oauth" oauth integration

  Scenario: Connecting an oauth integration signs in and then records it
    Given the background service is available to sign in
    When the developer runs "lns integration connect some-oauth"
    Then a verification URL and user code are shown
    And "some-oauth" is recorded under integrations in lns-policy.yaml
    And lns-policy.yaml carries no token material

  Scenario: Connecting an oauth integration fails clearly when the service is unavailable
    Given the background service is not available
    When the developer runs "lns integration connect some-oauth"
    Then the command fails noting the service is needed to sign in
    And "some-oauth" is not recorded in lns-policy.yaml

  Scenario: The catalog listing shows each integration's auth kind
    When the developer runs "lns integration list"
    Then "some-oauth" is listed as authenticating by oauth
    And "gitlab" is listed as authenticating by credential

  Scenario: Connecting a pkce integration opens the browser and then records it
    Given a user catalog declares the "some-pkce" pkce integration
    And the background service is available to sign in
    When the developer runs "lns integration connect some-pkce"
    Then the browser is opened to the authorization page
    And no user code is shown
    And "some-pkce" is recorded under integrations in lns-policy.yaml
    And lns-policy.yaml carries no credential material

  Scenario: Connecting a pkce integration fails clearly when the service is unavailable
    Given a user catalog declares the "some-pkce" pkce integration
    And the background service is not available
    When the developer runs "lns integration connect some-pkce"
    Then the command fails noting the service is needed to sign in
    And "some-pkce" is not recorded in lns-policy.yaml

  Scenario: The catalog listing shows a pkce integration as authenticating by oauth
    Given a user catalog declares the "some-pkce" pkce integration
    When the developer runs "lns integration list"
    Then "some-pkce" is listed as authenticating by oauth
