@todo
Feature: lns run --rm — auto-remove on exit
  --rm opts a run out of persistence: its state is removed when it ends.
  Nothing defaults to it — without --rm every run persists as a stopped
  run until removed.

  Scenario: a --rm run leaves nothing behind
    Given "lns run --rm" whose workload exits
    Then no stopped run remains for it
    And its run dir is gone

  Scenario: without --rm the run persists
    Given "lns run" whose workload exits
    Then a stopped run remains, restartable with "lns start"