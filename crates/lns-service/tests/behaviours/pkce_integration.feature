Feature: lns-service pkce integration sign-in
  Some integrations authenticate by a browser-redirect OAuth 2.0
  authorization-code flow with PKCE rather than a device code or a pasted
  secret. A pkce integration carries an `oauth` block with `flow: pkce` (an
  authorization endpoint and a token endpoint, no client id and no device
  endpoint); the background service builds an authorization URL with a PKCE
  challenge and a state nonce, binds a transient loopback callback listener,
  opens the browser, and exchanges the returned code for the provider's key.
  Unlike a device-flow grant the result is a durable credential value — there
  is no refresh and no expiry, so it stays armed across runs. The flow rides
  the existing connect/hold/notify pipeline: an un-connected pkce integration
  seeds its placeholder unarmed, a request carrying it is held and the
  approval surface offers to connect, and accepting runs the browser sign-in
  instead of asking for a value. On success the integration is connected live
  and the key is armed; cancel, timeout, or a rejected exchange fails the held
  request without persisting. (The PKCE proof — the exchanged verifier hashing
  to the challenge — a forged-callback state mismatch, and a listener that
  can't bind are pinned at the unit layer in `oauth/mod.rs`.)

  Scenario: First use of an unconnected pkce integration signs in via browser, connects, and arms it
    Given an unconnected "some-pkce" oauth integration using the pkce flow whose sign-in will complete
    When a workload request carries the "some-pkce" placeholder
    Then the request is held and a "connect to some-pkce" prompt is presented
    When the developer accepts the prompt
    Then the browser is opened to the authorization page
    And a credential is stored for "some-pkce"
    And the "some-pkce" integration is connected live
    And the held request is released for injection

  Scenario: A pkce sign-in the developer cancels fails the held request and stores nothing
    Given an unconnected "some-pkce" oauth integration using the pkce flow
    When a workload request carries the "some-pkce" placeholder
    And the developer accepts the prompt
    And the developer cancels the sign-in
    Then the held request is failed at the boundary
    And no credential is stored for "some-pkce"
    And the "some-pkce" integration is not connected

  Scenario: A pkce sign-in whose callback never arrives times out and stores nothing
    Given an unconnected "some-pkce" oauth integration whose callback never arrives
    When a workload request carries the "some-pkce" placeholder
    And the developer accepts the prompt
    Then the held request is failed at the boundary
    And no credential is stored for "some-pkce"

  Scenario: A pkce code exchange that the provider rejects fails the held request and stores nothing
    Given an unconnected "some-pkce" oauth integration whose code exchange will fail
    When a workload request carries the "some-pkce" placeholder
    And the developer accepts the prompt
    Then the held request is failed at the boundary
    And no credential is stored for "some-pkce"

  Scenario: A pkce-obtained key stays armed on the next run without signing in again
    Given "some-pkce" was connected and its credential stored
    When a new run starts and a workload request carries the "some-pkce" placeholder
    Then the request is injected without a sign-in prompt
