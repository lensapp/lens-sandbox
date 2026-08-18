@todo
Feature: prune sweeps stopped runs
  `lns sandbox prune --force` sweeps all stopped runs and orphan run dirs
  before the reconstructible cache — the one command that sweeps it all
  away.

  Scenario: prune removes all stopped runs
    Given two stopped runs and one running run
    When I run "lns sandbox prune --force"
    Then both stopped runs are removed
    And the running run is untouched

  Scenario: pre-feature orphaned run dirs are swept
    Given a run dir with no run record
    When I run "lns sandbox prune --force"
    Then the orphaned dir is removed