Feature: inspecting and forgetting per-workload connector grants
  Connecting a connector to a directory binds its value on the machine;
  letting a particular workload spend that value is a separate decision,
  remembered per project, workload, and connector. `lns connector grants`
  shows what this project has granted so the answer isn't only visible in
  the approval window, and `lns connector revoke` takes a grant back so the
  connector's next use asks again. `lns connector disconnect` forgets the
  connector's grants here as part of dropping it from the policy.

  Grants are per-machine, so no grant data reaches `lns-local-mixin.yaml`; these
  scenarios use an arbitrary user-declared connector, so nothing here pins
  a shipped service.

  Background:
    Given this project connects "some-provider"

  Scenario: Listing shows the workload, connector, and verdict this project granted
    Given the workload "def:/work/app" was granted "some-provider"
    When the user runs connector command "grants"
    Then the listing shows "def:/work/app" holding "allow" for "some-provider"

  Scenario: The listing names its columns
    Given the workload "def:/work/app" was granted "some-provider"
    When the user runs connector command "grants"
    Then the listing is headed "WORKLOAD  CONNECTOR  VERDICT"

  Scenario: A workload composed with a mixin lists as the composition the user typed
    Given the workload "def:/work/app" composed with mixin "ghcr.io/acme/tools@sha256:abc" was granted "some-provider"
    When the user runs connector command "grants"
    Then the listing shows "def:/work/app + ghcr.io/acme/tools@sha256:abc" holding "allow" for "some-provider"
    And the output survives a pipe

  Scenario: A script still matches a composed workload on its stored key
    Given the workload "def:/work/app" composed with mixin "ghcr.io/acme/tools@sha256:abc" was granted "some-provider"
    When the user runs connector command "grants --format json"
    Then the json keeps the composed key of "def:/work/app" and "ghcr.io/acme/tools@sha256:abc" verbatim

  Scenario: A workload that declined the connector is listed as a deny
    Given the workload "def:/work/app" was denied "some-provider"
    When the user runs connector command "grants"
    Then the listing shows "def:/work/app" holding "deny" for "some-provider"

  Scenario: Listing reports plainly when this project has granted nothing
    When the user runs connector command "grants"
    Then the output reports no grants for this project

  Scenario: Another project's grants are not this project's business
    Given the project "/work/other" granted "some-provider"
    When the user runs connector command "grants"
    Then the output reports no grants for this project

  Scenario: Listing every project on this machine needs --all
    Given the project "/work/other" granted "some-provider"
    When the user runs connector command "grants --all"
    Then the listing names the project "/work/other"

  Scenario: Revoking forgets a connector's grants so its next use asks again
    Given the workload "def:/work/app" was granted "some-provider"
    When the user runs connector command "revoke some-provider"
    Then the output reports 1 grant forgotten
    And this project holds no grant for "some-provider"

  Scenario: Revoking here leaves another project's grant for the same connector alone
    Given the workload "def:/work/app" was granted "some-provider"
    And the project "/work/other" granted "some-provider"
    When the user runs connector command "revoke some-provider"
    Then the project "/work/other" still holds its grant for "some-provider"

  Scenario: Revoking a connector that was never granted fails rather than reporting success
    When the user runs connector command "revoke some-provider"
    Then the command fails noting there is nothing to forget

  Scenario: Disconnecting a connector forgets its grants here too
    Given the workload "def:/work/app" was granted "some-provider"
    When the user runs connector command "disconnect some-provider"
    Then the output reports the grants it forgot
    And this project holds no grant for "some-provider"

  Scenario: Connecting says so when a workload here has declined the connector
    Given a user catalog declares the "some-provider" credential connector
    And the background service is available to sign in
    And the workload "def:/work/app" was denied "some-provider"
    When the developer runs "lns connector connect some-provider"
    Then the output points at revoking the standing decline for "some-provider"

  Scenario: Connecting stays quiet when nothing here has declined the connector
    Given a user catalog declares the "some-provider" credential connector
    And the background service is available to sign in
    And the workload "def:/work/app" was granted "some-provider"
    When the developer runs "lns connector connect some-provider"
    Then the output says nothing about a standing decline

  Scenario: A disconnect that cannot forget the grants leaves the connector connected
    Given the workload "def:/work/app" was granted "some-provider"
    And the grant sidecar cannot be updated
    When the user runs connector command "disconnect some-provider"
    Then the exit code is 1
    And this project still connects "some-provider"

  Scenario: Listing grants as JSON names the project each grant belongs to
    Given the workload "def:/work/app" was granted "some-provider"
    When the user runs connector command "grants --format json"
    Then the output is a JSON array of 1 rows
    And JSON row 0 has "workload" set to "def:/work/app"
    And JSON row 0 has "connector" set to "some-provider"
    And JSON row 0 has "verdict" set to "allow"
    And JSON row 0 has a non-empty "project"

  Scenario: A project that has granted nothing lists as an empty JSON array
    When the user runs connector command "grants --format json"
    Then the output is an empty JSON array

  Scenario: A declined grant is visible to a script, not filtered out
    Given the workload "def:/work/app" was denied "some-provider"
    When the user runs connector command "grants --format json"
    Then the output is a JSON array of 1 rows
    And JSON row 0 has "verdict" set to "deny"
