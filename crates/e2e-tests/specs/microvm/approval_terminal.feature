# Parked under specs/microvm/ (not the globbed features/ dir): both scenarios
# need a guest that holds a request while a second terminal answers it, and the
# harness drives one command at a time — it can boot a guest and it can run
# `lns approval`, but it cannot hold a workload at an undecided destination
# across the two. Like microvm_egress.feature, the interactive ask/answer flow
# needs a decision made while the request waits. The store, the entry
# lifecycle, and the CLI grammar are pinned at Layer 2
# (approval_entries.feature, approval_cli.feature); this file is the live
# terminal-to-guest path. See CLAUDE.md "Out of scope".
@microvm
Feature: answering a real run's approvals from the terminal
  The approval window and `lns approval` are two views of one store the
  service owns, so an answer given at the terminal is the same answer the
  window would have delivered. This proves it across process boundaries
  against a booted guest: a real request with no matching rule raises an
  entry, the terminal lists it, the terminal answers it, and the run gets
  the decision. A headless service has no window at all, which makes the
  terminal the only surface — a run that would once have needed a
  pre-authored rule can be answered while it waits.

  Scenario: an answer given at the terminal resolves the run's pending question
    Given the LNS service is running
    And a workload in a booted guest reaches a destination no rule decides
    When the user runs "lns approval ls"
    Then the exit code is 0
    And the output names that destination as undecided
    When the user answers that entry with "always-allow"
    Then the exit code is 0
    And the workload's request proceeds
    And "lns approval ls" reports that destination as always allowed

  Scenario: a headless service asks at the terminal instead of refusing the run
    Given the LNS service is running headless
    And a workload in a booted guest reaches a destination no rule decides
    When the user runs "lns approval ls"
    Then the exit code is 0
    And the output names that destination as undecided
    When the user answers that entry with "always-allow"
    Then the workload's request proceeds
