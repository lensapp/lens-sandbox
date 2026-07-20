@serial
Feature: lns-service credential storage backends
  Credential value decisions are protected at rest. When the host offers
  an OS-native keychain — the macOS Keychain, Windows Credential
  Manager, or a Linux Secret Service — lns-service keeps the whole
  per-machine credential state in a single keychain item and writes no
  secret material to disk. When no keychain is reachable (headless
  Linux, CI), it falls back to the plaintext JSON file with a warning.
  The backend is chosen once at startup; `LNS_CREDENTIALS_PATH` always
  forces the file backend at that path, keeping tests and scripted
  environments hermetic. A keychain item emits no file events, so
  changes made through the service apply to running sessions in-process;
  the file watcher remains a file-backend feature.

  These scenarios use synthetic "some-provider" / "some-oauth" fixtures;
  nothing here pins a shipped service.

  Scenario: A credential decision is persisted to the OS keychain, not to disk
    Given the OS keychain is reachable
    And a workload is running with the seeded "some-provider" placeholder
    When the developer stores a value for "some-provider" at the credential card
    Then the credential state lands in the OS keychain as a single item
    And no plaintext credentials file is written

  Scenario: Credential storage falls back to the plaintext file when no keychain is reachable
    Given no OS keychain is reachable
    When the service selects its credential backend
    Then the plaintext JSON file backend is selected
    And a warning notes that credential values will be stored in plaintext

  Scenario: LNS_CREDENTIALS_PATH forces the file backend even when a keychain is reachable
    Given the OS keychain is reachable
    And LNS_CREDENTIALS_PATH points at a custom path
    When the service selects its credential backend
    Then the plaintext JSON file backend at that path is selected
    And the OS keychain is never probed

  Scenario: A sign-in takes effect immediately under the keychain backend
    Given the keychain backend is active
    And a workload is running with an unconnected "some-oauth" integration
    When a device sign-in for "some-oauth" completes
    Then the running session arms the some-oauth token set without a restart
    And the token set lands in the OS keychain

  Scenario: An existing plaintext credentials file is ignored under the keychain backend
    Given the keychain backend is active with no stored state
    And a plaintext credentials file holds a "stored" entry for "some-provider"
    When the workload sends a request carrying the some-provider placeholder
    Then a credential card appears for "some-provider"
    And the plaintext credentials file is left untouched

  @todo
  Scenario: Revoking a value decision removes it and re-arms the prompt without a restart
    Given the keychain backend is active
    And a workload is running with a "stored" credential rule for "some-provider"
    When the developer revokes the "some-provider" credential
    Then the "some-provider" entry is removed from the credential state
    And a subsequent request carrying the some-provider placeholder fires a fresh credential card

  @todo
  Scenario: Revoking under the file backend behaves the same
    Given the file backend is active
    And a workload is running with a "stored" credential rule for "some-provider"
    When the developer revokes the "some-provider" credential
    Then the "some-provider" entry is removed from the credential state
    And a subsequent request carrying the some-provider placeholder fires a fresh credential card

  @todo
  Scenario: Service status reports the active credential backend
    Given the keychain backend is active
    When a status request is served
    Then the response names the OS keychain as the credential backend
