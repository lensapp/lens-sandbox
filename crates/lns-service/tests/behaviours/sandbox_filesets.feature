Feature: a sandbox's declared filesets are planned into the launch
  A sandbox definition may ship files via spec.filesets. The service
  plans them into the run: a published sandbox's fileset refs join the
  resolved plan (materialized into the guest by the fileset-pull
  machinery), a local definition's path filesets are walked into
  guest-write specs at plan time, and inline filesets are lowered directly
  from the sandbox definition — all as launch-time snapshots. The trust
  story is digest pinning plus disclosure, not signatures: a published
  sandbox whose fileset ref is not digest-pinned (or that smuggles a
  local path) is refused. The absence of any signature warning on a
  fileset-carrying sandbox run is pinned end to end in the Layer 1
  suite.

  Scenario: planning a published sandbox carries its fileset refs into the resolved plan
    Given a published sandbox declaring a digest-pinned fileset at "/root/.agent/skills"
    When the sandbox is planned
    Then the resolved plan carries the fileset ref at "/root/.agent/skills"

  Scenario: a published sandbox with a floating fileset ref is refused
    Given a published sandbox declaring a fileset by floating tag
    When the sandbox is planned
    Then the plan is refused naming the unpinned fileset ref

  Scenario: a published sandbox smuggling a local path fileset is refused
    Given a published sandbox declaring a local path fileset
    When the sandbox is planned
    Then the plan is refused naming the local path

  Scenario: planning a local definition snapshots each path fileset into guest-write specs
    Given a local definition declaring a path fileset containing "prompts.md" at "/root/.agent/skills"
    When the local definition is planned
    Then the plan carries a guest-write spec for "/root/.agent/skills/prompts.md"

  Scenario: a fileset's files transfer to the workload user by default
    Given a local definition declaring a path fileset containing "state.json" at "/home/sandbox"
    When the local definition is planned
    Then the plan ships a chown manifest listing "/home/sandbox/state.json"
    And the plan ships a chown manifest listing "/home/sandbox"

  Scenario: an owner root fileset ships no chown manifest
    Given a local definition declaring a root-owned path fileset containing "prompts.md" at "/opt/skills"
    When the local definition is planned
    Then the plan ships no chown manifest

  @todo
  Scenario: planning an inline fileset writes its exact content beneath the mount path
    Given a sandbox declaring inline file ".claude/settings.json" with content `{"enabled":true}` at "/home/sandbox"
    When the sandbox is planned
    Then the plan carries an inline guest-write spec for "/home/sandbox/.claude/settings.json" with content `{"enabled":true}`
    And the plan ships a chown manifest listing "/home/sandbox/.claude"
    And the plan ships a chown manifest listing "/home/sandbox/.claude/settings.json"
    And the plan ships a chown manifest listing "/home/sandbox"

  @todo
  Scenario: a root-owned inline fileset stays outside workload ownership
    Given a sandbox declaring root-owned inline file "mcp.json" at "/etc/agent"
    When the sandbox is planned
    Then the plan carries an inline guest-write spec for "/etc/agent/mcp.json"
    And the plan ships no chown manifest

  @todo
  Scenario: a published sandbox may carry inline files without a pinned fileset ref
    Given a published sandbox declaring an inline file at "/home/sandbox"
    When the sandbox is planned
    Then the plan accepts the inline fileset without a fileset ref
