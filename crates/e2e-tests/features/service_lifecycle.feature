Feature: Lens Sandbox tray-resident background service

  @gui
  Scenario: lns service start brings up the background service and the tray icon
    Given no Lens Sandbox service is running
    And no Lens Sandbox tray icon is visible
    When I run `lns service start`
    Then a Lens Sandbox tray icon appears in my OS tray
    And `lns service start` exits successfully
    And the tray icon remains visible after the command exits

  Scenario: lns service start is a no-op when the service is already running
    Given the Lens Sandbox service is running
    When I run `lns service start` again
    Then no second service is started
    And no second tray icon appears
    And `lns service start` exits successfully, reporting that the service is already running

  @gui
  Scenario: The tray icon outlives the originating terminal
    Given the Lens Sandbox tray icon is visible
    When I close the terminal that ran `lns service start`
    Then the tray icon remains visible
    And the service continues running

  @gui
  Scenario: The user can quit Lens Sandbox from the tray
    Given the Lens Sandbox tray icon is visible
    When I select "Quit Lens Sandbox" from the tray menu
    Then the tray icon disappears
    And no Lens Sandbox service remains running

  @gui
  Scenario: The user can stop Lens Sandbox from the CLI
    Given the Lens Sandbox service is running
    When I run `lns service stop`
    Then the tray icon disappears
    And no Lens Sandbox service remains running
    And `lns service stop` exits successfully

  Scenario: lns service stop is a no-op when nothing is running
    Given no Lens Sandbox service is running
    When I run `lns service stop`
    Then `lns service stop` exits successfully, reporting that nothing was running

  Scenario: Commands that need the service fail clearly when it is not running
    Given no Lens Sandbox service is running
    When I run an `lns` command that requires the service
    Then the command exits with a non-zero status
    And the error message reads: "Lens Sandbox is not running. Run `lns service start` to start it."

  Scenario: Local-only commands work without the service
    Given no Lens Sandbox service is running
    When I run `lns version` or `lns help`
    Then the command completes successfully
    And no service is started as a side effect
    And no tray icon appears

  Scenario: lns service status reports the running service's details
    Given the Lens Sandbox service is running
    When I run `lns service status`
    Then the command reports that the service is running
    And the report includes the service's PID, uptime, and version

  Scenario: lns service status reports when no service is running
    Given no Lens Sandbox service is running
    When I run `lns service status`
    Then the command reports that no service is running

  Scenario: Two terminals see the same running service
    Given the Lens Sandbox service is running
    When I run `lns service status` from one terminal
    And later run `lns service status` from another terminal
    Then both invocations report the same PID
    And the second invocation reports a strictly greater uptime than the first

  Scenario: Concurrent CLI invocations are both served by the same instance
    Given the Lens Sandbox service is running
    When two `lns service status` commands run concurrently from different terminals
    Then both observe consistent state from the same service instance
    And neither invocation corrupts or races the other

  Scenario: A CLI and service built from the same tree pass the handshake silently
    Given the Lens Sandbox service is running
    When I run an `lns` command that requires the service
    Then the command completes successfully
    And the output contains no build or protocol drift warning
