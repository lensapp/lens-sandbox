Feature: a sandbox's declared filesets are planned into the launch
  A sandbox definition may ship files via spec.filesets. A fileset is
  not a separate artifact: a path entry's files travel in a layer of the
  document's own artifact. The service plans them into the run: a
  pulled document's path filesets resolve to the layer of the artifact
  that shipped them, a local definition's path filesets are walked into
  guest-write specs at plan time, and inline filesets are lowered
  directly from the sandbox definition — all as launch-time snapshots.
  The trust story is digest pinning plus disclosure, not signatures: a
  published sandbox whose files no digest-pinned artifact ships is
  refused. The absence of any signature warning on a fileset-carrying
  sandbox run is pinned end to end in the Layer 1 suite.

  Scenario: planning a published sandbox points each path fileset at the layer that ships it
    Given a published sandbox declaring a path fileset at "/root/.agent/skills"
    When the sandbox is planned
    Then the resolved plan pulls "/root/.agent/skills" from layer 0 of the sandbox artifact

  Scenario: a mixin's path fileset is pulled from that mixin's own artifact
    Given a published sandbox whose mixin ships a path fileset at "/root/.agent/prompts"
    When the sandbox is planned
    Then the resolved plan pulls "/root/.agent/prompts" from the mixin artifact

  Scenario: a fileset the directory itself decided is never fetched from a registry
    Given a published sandbox whose decisions file ships a path fileset at "/root/.agent/notes"
    When the sandbox is planned
    Then the plan reads "/root/.agent/notes" from disk instead of pulling it
    And the plan is refused because no pinned artifact ships "/root/.agent/notes"

  Scenario: a published sandbox whose fileset source is a floating tag is refused
    Given a published sandbox whose fileset source is a floating tag
    When the sandbox is planned
    Then the plan is refused because no pinned artifact ships "/root/.agent/skills"

  Scenario: a published sandbox smuggling a local path fileset is refused
    Given a published sandbox declaring a local path fileset
    When the sandbox is planned
    Then the plan is refused because no pinned artifact ships "/root/.agent/skills"

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

  Scenario: planning an inline fileset writes its exact content beneath the mount path
    Given a sandbox declaring inline file ".claude/settings.json" with content `{"enabled":true}` at "/home/sandbox"
    When the sandbox is planned
    Then the plan carries an inline guest-write spec for "/home/sandbox/.claude/settings.json" with content `{"enabled":true}`
    And the plan ships a chown manifest listing "/home/sandbox/.claude"
    And the plan ships a chown manifest listing "/home/sandbox/.claude/settings.json"
    And the plan ships a chown manifest listing "/home/sandbox"

  Scenario: a root-owned inline fileset stays outside workload ownership
    Given a sandbox declaring root-owned inline file "mcp.json" at "/etc/agent"
    When the sandbox is planned
    Then the plan carries an inline guest-write spec for "/etc/agent/mcp.json"
    And the plan ships no chown manifest

  Scenario: a published sandbox may carry inline files without any layer
    Given a published sandbox declaring an inline file at "/home/sandbox"
    When the sandbox is planned
    Then the plan accepts the inline fileset without a packed layer

  Scenario: a hostPath fileset lands at its mountPath as a host-file write
    Given a definition declaring a hostPath fileset "/etc/gitconfig" at "/home/agent/.gitconfig"
    And the host file "/etc/gitconfig" exists with mode 0644
    When the host files are planned
    Then the plan carries a host-file write from "/etc/gitconfig" to "/home/agent/.gitconfig"

  Scenario: a home-rooted hostPath resolves against this machine's home
    Given a definition declaring a hostPath fileset "~/.gitconfig" at "/home/agent/.gitconfig"
    And this machine's home directory is "/home/some-user"
    And the host file "/home/some-user/.gitconfig" exists with mode 0644
    When the host files are planned
    Then the plan carries a host-file write from "/home/some-user/.gitconfig" to "/home/agent/.gitconfig"

  Scenario: an absent optional hostPath fileset is planned as nothing
    Given a definition declaring an optional hostPath fileset "/etc/gitconfig" at "/home/agent/.gitconfig"
    When the host files are planned
    Then the plan carries no guest-write spec

  Scenario: an absent required hostPath fileset refuses the plan
    Given a definition declaring a hostPath fileset "/etc/gitconfig" at "/home/agent/.gitconfig"
    When the host files are planned
    Then the plan is refused naming "/etc/gitconfig"

  Scenario: a published sandbox may declare a hostPath fileset but still not a local path fileset
    Given a published sandbox declaring a hostPath fileset "~/.gitconfig" at "/home/agent/.gitconfig"
    When the sandbox is planned
    Then the plan accepts the hostPath fileset
