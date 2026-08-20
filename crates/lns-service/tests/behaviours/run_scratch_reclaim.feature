Feature: run scratch reclaim — a finished run gives back its disk
  A run's scratch dir under the cache holds its upper.img and console.log.
  Ephemeral by default: exit reclaims the upper disk, removal reclaims the
  dir, and startup sweeps orphans. The audit chain lives in the data dir
  and is never touched.

  Scenario: A finished run gives back its scratch space
    Given a run has exited
    When the service reclaims the run
    Then the run's upper.img no longer exists
    And the run's console.log still exists

  Scenario: Removing an exited run reclaims its scratch dir
    Given a run has exited
    When the exited run is removed
    Then the run's scratch dir no longer exists

  Scenario: Prune reports what it reclaimed
    Given two exited runs with scratch dirs
    When the exited runs are pruned
    Then both scratch dirs are gone
    And the prune reports the reclaimed bytes

  Scenario: Prune keeps a running run's scratch dir
    Given a running run with a scratch dir
    And an exited run with a scratch dir
    When the exited runs are pruned
    Then only the exited run's scratch dir is gone

  Scenario: A scratch dir orphaned by a service crash is reclaimed
    Given a scratch dir whose run is not in the registry
    When the startup sweep runs
    Then that scratch dir is removed

  Scenario: The startup sweep never touches the audit chain
    Given a scratch dir whose run is not in the registry
    And that run's audit chain exists in the data dir
    When the startup sweep runs
    Then that run's audit chain still exists
