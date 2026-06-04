Feature: lns-service IPC protocol — one-shot requests
  The service responds to non-streaming IPC requests with
  deterministic Response variants. Each request kind has a documented
  response contract; these scenarios pin them against the in-process
  dispatcher so a regression surfaces without needing a real daemon.

  Scenario: Ping returns Pong
    Given a fresh service handler
    When a Ping request arrives
    Then the response is Pong

  Scenario: Status reports pid + version + uptime
    Given a service handler that has been running for at least 2 seconds
    When a Status request arrives
    Then the response is Status
    And the response pid matches the current process
    And the response version matches the lns-service package version
    And the response uptime is at least 2 seconds

  Scenario: Shutdown acknowledges with ShuttingDown
    Given a fresh service handler
    When a Shutdown request arrives
    Then the response is ShuttingDown

  Scenario: Unknown method surfaces a descriptive Error
    Given a fresh service handler
    When an Unknown request with method "no-such-method" arrives
    Then the response is Error
    And the error message contains "unknown method: no-such-method"

  Scenario: CancelRun for a non-existent run surfaces an Error
    Given a fresh service handler
    When a CancelRun request for run 99999 arrives
    Then the response is Error
    And the error message contains "no active run with id 99999"
