Feature: Every desktop client can read the service dashboard
  Scenario: Audit integrity warnings survive the desktop boundary
    Given a dashboard with an event and an integrity warning
    When a client reads the dashboard frames
    Then the dashboard contains the event and its original raw data
    And the integrity warning precedes the dashboard completion

  Scenario: Large timelines do not require an oversized IPC frame
    Given a dashboard whose events exceed one IPC frame in total
    When a client reads the dashboard frames
    Then every event arrives in a valid bounded IPC frame

  Scenario: A dashboard exposes the service's available approval actions
    Given a dashboard with a raw unanswered destination
    When a client reads the dashboard frames
    Then the destination retains its warning and persistent answer choices
