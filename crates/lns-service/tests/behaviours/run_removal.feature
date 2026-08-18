Feature: lns rm — remove a run
  Removing a stopped run deletes its run dir atomically — record and
  writable layer — releasing its name reservation and layer pins. A
  running run is refused unless forced.

  Scenario: removing a stopped run
    Given a stopped run named "reviewer"
    When I run "lns rm reviewer"
    Then it prints "reviewer"
    And it exits 0
    And the run's dir, record, and writable layer are gone
    And the name "reviewer" is free for a new run

  Scenario: removing a stopped run releases its layer pins
    Given a stopped run holding the only reference to a cached image
    When I remove the run
    Then the image is removable

  Scenario: removing a running run without -f is refused
    Given a running run named "reviewer"
    When I run "lns rm reviewer"
    Then it exits non-zero
    And the error says to stop it first with "lns stop reviewer" or force with "-f"
    And the run keeps running

  Scenario: rm -f stops and removes in one step
    Given a running run named "reviewer"
    When I run "lns rm -f reviewer"
    Then it exits 0
    And the run is killed and its state removed

  Scenario: removing an unknown run
    When I run "lns rm nosuch"
    Then it exits 1 with an error naming "nosuch"