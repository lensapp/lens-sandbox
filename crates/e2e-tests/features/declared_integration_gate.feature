Feature: a definition's credential requirements gate the launch before boot
  The unknown-id refusal for a declared integration, and the required
  credential-slot gates (an unbound value, or an oauth sign-in that cannot
  complete), all fire on the host before any microVM boots, so these wiring
  confirmations run virt-free through the real binaries: the CLI carries the
  definition on the wire, the service plans it, and the launch is refused or
  aborted with guidance. A bare `spec.integrations` id never gates here — it
  is offered reactively on first use.

  Scenario: an unknown declared integration refuses the launch
    Given a clean lns cache home
    And the Lens Sandbox service is running in that home
    And the project definition declares integration "some-unknown"
    When the user runs the sandbox definition
    Then the exit code is non-zero
    And the output contains "some-unknown"
    And the output contains "lns integration add"

  Scenario: a required credential slot with no bound value refuses the launch
    Given a clean lns cache home
    And the home's integration catalog declares "some-provider" managing "SOME_TOKEN"
    And the Lens Sandbox service is running in that home
    And the project definition requires credential "some-provider" injected as "SOME_TOKEN"
    When the user runs the sandbox definition
    Then the exit code is non-zero
    And the output contains "some-provider"
    And the output contains "injected as SOME_TOKEN"
    And the output contains "lns integration connect some-provider"

  Scenario: a required oauth credential slot with no machine grant aborts when the sign-in cannot complete
    Given a clean lns cache home
    And the home's integration catalog declares an oauth integration "some-oauth" signing in at "http://127.0.0.1:1"
    And the Lens Sandbox service is running in that home
    And the project definition requires credential "some-oauth" injected as "SOME_OAUTH_TOKEN"
    When the user runs the sandbox definition
    Then the exit code is non-zero
    And the output contains "needs a sign-in before the workload starts"
    And the output contains "launch aborted"
