@todo
Feature: a sandbox's declared filesets are planned into the launch
  A sandbox definition may ship files via spec.filesets. The service
  plans them into the run: a published sandbox's fileset refs join the
  resolved plan (materialized into the guest by the same machinery a
  bundle uses), and a local definition's path filesets are walked into
  guest-write specs at plan time — a launch-time snapshot. The trust
  story is digest pinning plus disclosure: a published sandbox whose
  fileset ref is not digest-pinned is refused, and no signature gate
  fires for sandboxes (pin + inspect disclosure + the runtime policy
  cage — signing is a tracked follow-up workstream).

  Scenario: planning a published sandbox carries its fileset refs into the resolved plan
    Given a published sandbox declaring a digest-pinned fileset at "/root/.agent/skills"
    When the sandbox is planned
    Then the resolved plan carries the fileset ref at "/root/.agent/skills"

  Scenario: a published sandbox with a floating fileset ref is refused
    Given a published sandbox declaring a fileset by floating tag
    When the sandbox is planned
    Then the plan is refused naming the unpinned fileset ref

  Scenario: planning a local definition snapshots each path fileset into guest-write specs
    Given a local definition declaring a path fileset containing "prompts.md" at "/root/.agent/skills"
    When the local definition is planned
    Then the plan carries a guest-write spec for "/root/.agent/skills/prompts.md"

  Scenario: no signature gate fires for a sandbox with filesets
    Given a published sandbox declaring a digest-pinned fileset at "/root/.agent/skills"
    When the sandbox is planned
    Then no signature verdict is consulted or recorded for it
