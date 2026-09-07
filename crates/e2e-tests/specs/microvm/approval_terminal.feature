# Parked under specs/microvm/ (not the globbed features/ dir): the answer
# decides the next request, so both scenarios end by proving a destination is
# reachable once allowed, and the harness has no deterministic host-side
# endpoint the guest can reach — the reason microvm_egress.feature defers its
# own allow-path. A live guest across commands is not the blocker: a detached
# run plus `exec` already provides one. The store, the entry lifecycle, and
# the CLI grammar are pinned at Layer 2 (approval_entries.feature,
# approval_cli.feature); this file is the live terminal-to-guest path.
# See CLAUDE.md "Out of scope".
@microvm
Feature: answering a real run's approvals from the terminal
  The approval window and `lns approval` are two views of one store the
  service owns, so an answer given at the terminal is the same answer the
  window would have delivered. This proves it across process boundaries
  against a booted guest: a real request with no matching rule raises an
  entry, the terminal lists it, the terminal answers it, and the run gets
  the decision. Answering never replays the request that raised the entry
  — that call failed when nothing decided it, so the answer decides the
  next one (cli-spec §3.7). A headless service has no window at all,
  which makes the terminal the only surface: a run that would once have
  needed a pre-authored rule can be answered without one.

  Scenario: an answer given at the terminal decides the run's next request
    Given the LNS service is running
    And a workload in a booted guest reaches a destination no rule decides
    When the user runs "lns approval ls"
    Then the exit code is 0
    And the output names that destination as undecided
    When the user answers that entry with "always-allow"
    Then the exit code is 0
    And "lns approval ls" reports that destination as always allowed
    And the workload's next request to that destination proceeds

  Scenario: a headless service asks at the terminal instead of refusing the run
    Given the LNS service is running headless
    And a workload in a booted guest reaches a destination no rule decides
    When the user runs "lns approval ls"
    Then the exit code is 0
    And the output names that destination as undecided
    When the user answers that entry with "always-allow"
    Then the workload's next request to that destination proceeds
