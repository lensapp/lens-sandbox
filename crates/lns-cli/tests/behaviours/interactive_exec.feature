Feature: interactive exec sessions from the CLI
  `lns exec` keeps explicit stdin and PTY flags while accepting
  commands without a `--` separator and surfacing inactive-run failures.

  @todo
  Scenario: exec remains non-interactive by default
    Given an active run named "reviewer"
    When the user runs "lns exec reviewer echo hello"
    Then the exit code is 0
    And the output contains "hello"
    And host stdin is not forwarded
    And no PTY is allocated

  @todo
  Scenario: interactive mode forwards piped input without a PTY
    Given an active run named "reviewer"
    And "hello" is available on host stdin
    When the user runs "lns exec -i reviewer cat"
    Then the exec command receives "hello" on stdin
    And no PTY is allocated
    And the output contains "hello"

  @todo
  Scenario: TTY mode allocates a PTY without forwarding host stdin
    Given an active run named "reviewer"
    When the user runs "lns exec -t reviewer sh"
    Then the exec command has a PTY
    And host stdin is not forwarded

  @todo
  Scenario: interactive TTY mode supports terminal applications
    Given an active run named "reviewer"
    When the user runs "lns exec -it reviewer sh"
    Then host stdin is forwarded through an allocated PTY
    And the user receives a live shell prompt
    And raw-mode terminal programs can run
    And terminal output is displayed live

  Scenario: exec help exposes its terminal controls
    When the user runs "lns exec --help"
    Then the exit code is 0
    And the output contains "--interactive"
    And the output contains "--tty"
    And the output contains "--detach-keys"

  @todo
  Scenario: exec requires an active run
    Given no active run is named "ghost"
    When the user runs "lns exec -it ghost sh"
    Then the command fails with an exit code other than 0
    And the output contains "no such run: ghost"
    And no run is created
