Feature: connecting connectors from the CLI
  `lns connector connect <id>` binds a connector's per-machine value
  decision. A credential connector binds through the approval-window
  card — use the host-detected value, store one, or deny — and an `oauth`
  connector by an interactive sign-in the background service drives, so
  connecting one shows the verification URL and code. Either way the
  decision lands in the per-machine credential store, never in
  `lns-local-mixin.yaml`; the connection is recorded for the directory in
  the per-machine sidecar only once the bind or sign-in completes.
  `lns connector list` shows, per connector, how it signs in and whether
  it is connected here. `--project <PATH>` acts on another project
  directory instead of the current one.

  These scenarios use arbitrary user-declared connectors, so nothing
  here pins a shipped service.

  Background:
    Given a user catalog declares the "some-oauth" oauth connector

  Scenario: Connecting a credential connector binds the value decision through the approval window
    Given a user catalog declares the "some-provider" credential connector
    And the background service is available to sign in
    When the developer runs "lns connector connect some-provider"
    Then the output describes binding a credential value
    And "some-provider" is recorded as connected for this project
    And lns-local-mixin.yaml carries no credential material

  Scenario: Connecting a credential connector fails clearly when the service is unavailable
    Given a user catalog declares the "some-provider" credential connector
    And the background service is not available
    When the developer runs "lns connector connect some-provider"
    Then the command fails noting the service is needed to bind
    And "some-provider" is not recorded as connected

  Scenario: Connecting an oauth connector signs in and then records it
    Given the background service is available to sign in
    When the developer runs "lns connector connect some-oauth"
    Then a verification URL and user code are shown
    And "some-oauth" is recorded as connected for this project
    And lns-local-mixin.yaml carries no token material

  Scenario: Connecting an oauth connector fails clearly when the service is unavailable
    Given the background service is not available
    When the developer runs "lns connector connect some-oauth"
    Then the command fails noting the service is needed to sign in
    And "some-oauth" is not recorded as connected

  Scenario: The catalog listing shows each connector's auth kind
    When the developer runs "lns connector list"
    Then "some-oauth" is listed as authenticating by oauth
    And "gitlab" is listed as authenticating by credential

  Scenario: Connecting a pkce connector opens the browser and then records it
    Given a user catalog declares the "some-pkce" pkce connector
    And the background service is available to sign in
    When the developer runs "lns connector connect some-pkce"
    Then the browser is opened to the authorization page
    And no user code is shown
    And "some-pkce" is recorded as connected for this project
    And lns-local-mixin.yaml carries no credential material

  Scenario: Connecting a pkce connector fails clearly when the service is unavailable
    Given a user catalog declares the "some-pkce" pkce connector
    And the background service is not available
    When the developer runs "lns connector connect some-pkce"
    Then the command fails noting the service is needed to sign in
    And "some-pkce" is not recorded as connected

  Scenario: The catalog listing shows a pkce connector as authenticating by oauth
    Given a user catalog declares the "some-pkce" pkce connector
    When the developer runs "lns connector list"
    Then "some-pkce" is listed as authenticating by oauth

  Scenario: The catalog listing as JSON labels each connector's source and auth kind
    Given a user catalog declares the "some-provider" credential connector
    When the user runs connector command "list --format json"
    Then the JSON row for "some-provider" has "source" set to "user"
    And the JSON row for "some-provider" has "authKind" set to "credential"

  Scenario: The listing says whether each connector is connected in this project
    Given a user catalog declares the "some-provider" credential connector
    And "some-provider" is already connected in this project
    When the developer runs "lns connector list"
    Then the output contains "CONNECTED"
    And "some-provider" is listed as connected here
    And "gitlab" is listed as not connected here

  Scenario: The JSON listing carries the same answer for a script
    Given a user catalog declares the "some-provider" credential connector
    And "some-provider" is already connected in this project
    When the user runs connector command "list --format json"
    Then the JSON row for "some-provider" has "connected" set to true
    And the JSON row for "gitlab" has "connected" set to false

  Scenario: --project acts on another project directory
    When the developer runs "lns connector connect some-oauth --project /other"
    Then "some-oauth" is recorded as connected for the project at "/other"
    And "some-oauth" is not recorded as connected

  Scenario: --policy is gone; the flag names a project, not a file
    When I run "lns connector connect some-oauth --policy team.yaml"
    Then the exit code is 2
    And the output contains "unexpected argument"

  Scenario: adding a connector that claims a domain another already claims is refused
    Given a user catalog declares the "some-provider" credential connector
    When the developer adds a connector claiming "api.some-provider.example"
    Then the command fails naming "some-provider" as the connector that already claims it
    And the catalog is left unchanged
