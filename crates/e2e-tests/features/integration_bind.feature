Feature: a credential bind fails closed when no approval window can appear
  The connect-time value decision is card-driven: the service raises the
  approval-window card and waits for the developer to decide. A service
  running headless (LNS_HEADLESS=1, or no display) can never show that
  card, so the bind fails immediately with the reason instead of holding
  the CLI until a timeout. The interactive happy path needs a human
  decision on the card and stays a manual check.

  Scenario: connect fails cleanly when the service runs headless
    Given a clean lns cache home
    And the home's integration catalog declares "some-provider" managing "SOME_TOKEN"
    And the Lens Sandbox service is running headless in that home
    When the user connects integration "some-provider"
    Then the exit code is non-zero
    And the output contains "no display"
    And the output contains "did not complete"
