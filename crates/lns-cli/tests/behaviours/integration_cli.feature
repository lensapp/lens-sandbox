Feature: connecting integrations from the CLI
  `lns integration connect <id>` binds an integration's per-machine value
  decision. A credential integration binds through the approval-window
  card — use the host-detected value, store one, or deny — and an `oauth`
  integration by an interactive sign-in the background service drives, so
  connecting one shows the verification URL and code. Either way the
  decision lands in the per-machine credential store, never in
  `lns-policy.yaml`; the id is recorded under `integrations:` in that
  directory's policy only once the bind or sign-in completes.
  `lns integration list` shows, per integration, whether it authenticates
  by sign-in or a value.

  These scenarios use arbitrary user-declared integrations, so nothing
  here pins a shipped service.

  Background:
    Given a user catalog declares the "some-oauth" oauth integration

  Scenario: Connecting a credential integration binds the value decision through the approval window
    Given a user catalog declares the "some-provider" credential integration
    And the background service is available to sign in
    When the developer runs "lns integration connect some-provider"
    Then the output describes binding a credential value
    And "some-provider" is recorded under integrations in lns-policy.yaml
    And lns-policy.yaml carries no credential material

  Scenario: Connecting a credential integration fails clearly when the service is unavailable
    Given a user catalog declares the "some-provider" credential integration
    And the background service is not available
    When the developer runs "lns integration connect some-provider"
    Then the command fails noting the service is needed to bind
    And "some-provider" is not recorded in lns-policy.yaml

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

  Scenario: Revoking an integration clears its per-machine value decision
    Given the background service is available
    When the developer runs "lns integration revoke some-oauth"
    Then the command confirms the "some-oauth" value decision is cleared
    And lns-policy.yaml is unchanged

  Scenario: Revoking an integration fails clearly when the service is unavailable
    Given the background service is not available
    When the developer runs "lns integration revoke some-oauth"
    Then the command fails noting the service is needed to revoke
