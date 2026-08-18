Feature: lns start — restart a stopped run
  A run that ends becomes a stopped run: restartable until removed.
  Start replays the launch config verbatim from the run record, re-resolves
  network policy and credentials live, and fails closed on any conflict —
  config frozen, policy live, conflicts fail closed.

  Scenario: starting an already-running run is an idempotent success
    Given a run named "reviewer" that is running
    When I run "lns start reviewer"
    Then it exits 0
    And the run is unchanged

  Scenario: unknown run lists what is startable
    Given stopped runs named "reviewer" and "builder"
    When I run "lns start nosuch"
    Then it exits 1
    And the error names "reviewer" and "builder" as the stopped runs that exist

  Scenario: unknown run with nothing stopped
    Given no stopped runs
    When I run "lns start nosuch"
    Then it exits 1
    And the error says there are no stopped runs

  Scenario: launch config replays verbatim from the run record
    Given a stopped run started from an lns.yaml that has since changed
    When I start it
    Then it boots with the recorded image, command, env, mounts, and ports
    And the changed lns.yaml has no effect on it

  Scenario: a taken host port fails the start closed
    Given a stopped run that published host port 8080
    And another process now holds host port 8080
    When I start it
    Then it exits non-zero with an error naming port 8080
    And the run remains stopped with its state untouched

  Scenario: a volume held by another run fails the start closed
    Given a stopped run that used volume "data"
    And a running run currently holds volume "data"
    When I start it
    Then it exits non-zero with an error naming "data" and its holder
    And the run remains stopped

  Scenario: a missing bind source fails the start closed
    Given a stopped run with a bind whose host directory no longer exists
    When I start it
    Then it exits non-zero with an error naming the missing path

  Scenario: a corrupt or missing writable layer is a specific error
    Given a stopped run whose upper.img has been deleted from its run dir
    When I start it
    Then it exits non-zero with an error saying the run's state is damaged