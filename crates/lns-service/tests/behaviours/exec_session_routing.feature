Feature: routing exec sessions within a run
  Every exec session is isolated from the run's primary session and from
  concurrent exec sessions for lifecycle and terminal-control traffic.

  Scenario: exec works while the primary session is attached
    Given an active run named "reviewer"
    And its primary session is attached to another client
    When the user runs "lns exec -it reviewer sh"
    Then the exec session is opened and its input routes to it alone
    And the primary session remains attached and usable

  Scenario: a resize targets one exec session
    Given an active run named "reviewer"
    And its primary session is running
    And two interactive exec sessions are active
    When the first exec client resizes its terminal
    Then only the first exec session receives the new dimensions
    And the primary session is unaffected
    And the second exec session remains usable

  Scenario: a signal targets one exec session
    Given an active run named "reviewer"
    And its primary session is running
    And two interactive exec sessions are active
    When the first exec client sends SIGINT
    Then only the first exec session receives SIGINT
    And the primary session is unaffected
    And the second exec session remains usable

  Scenario: the detach chord closes only its exec session
    Given an active run named "reviewer"
    And its primary session is running
    And two interactive exec sessions are active
    When the user enters the detach chord in the first exec session
    Then the first exec session is terminated
    And its CLI returns successfully
    And the primary session remains running
    And the second exec session remains usable

  Scenario: an unexpected exec disconnect is isolated
    Given an active run named "reviewer"
    And its primary session is running
    And two interactive exec sessions are active
    When the first exec client disconnects unexpectedly
    Then only the first exec session is cancelled
    And the primary session remains running
    And the second exec session remains usable
