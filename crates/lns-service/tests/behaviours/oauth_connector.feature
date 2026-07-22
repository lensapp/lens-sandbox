Feature: lns-service oauth connector sign-in
  Some connectors authenticate by an interactive sign-in rather than a
  pasted secret. An `oauth` connector carries a device-flow
  configuration (client id, scopes, the device-authorization and token
  endpoints) in the catalog; its credential value is a self-renewing
  token set obtained by the OAuth 2.0 Device Authorization Grant. The flow
  rides the existing connect/hold/notify pipeline: an un-connected oauth
  connector seeds its placeholder unarmed, a request carrying it is held
  and the approval surface offers to connect, and accepting runs the device
  sign-in instead of asking for a value. On success the connector is
  connected live and the token set is armed; denial or expiry fails the
  held request without persisting. (The run-start refresh of an expired
  grant, and dropping a grant that can no longer be refreshed, are pinned
  at the unit layer in `oauth/mod.rs`.)

  Scenario: First use of an unconnected oauth connector signs in, connects, and arms it
    Given an unconnected "some-oauth" oauth connector whose sign-in will complete
    When a workload request carries the "some-oauth" placeholder
    Then the request is held and a "connect to some-oauth" prompt is presented
    When the developer accepts the prompt
    Then the "some-oauth" connector is connected live
    And a token set is stored for "some-oauth"
    And the held request is released for injection

  Scenario: A device sign-in that expires fails the held request and stores nothing
    Given an unconnected "some-oauth" oauth connector whose sign-in will expire
    When a workload request carries the "some-oauth" placeholder
    And the developer accepts the prompt
    Then the held request is failed at the boundary
    And no token set is stored for "some-oauth"
    And the "some-oauth" connector is not connected

  Scenario: A device sign-in denied in the browser stores nothing
    Given an unconnected "some-oauth" oauth connector whose sign-in will be denied
    When a workload request carries the "some-oauth" placeholder
    And the developer accepts the prompt
    Then the held request is failed at the boundary
    And no token set is stored for "some-oauth"

  Scenario: A user-catalog oauth connector signs in through the same flow
    Given an unconnected "acme" oauth connector whose sign-in will complete
    When a workload request carries the "acme" placeholder
    And the developer accepts the prompt
    Then the "acme" connector is connected live
    And a token set is stored for "acme"
