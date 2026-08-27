Feature: a sandbox's declared filesets are planned into the launch
  A sandbox definition may ship files via spec.filesets. The service
  plans them into the run: a published document's path filesets arrive
  packed into layers of the very artifact the user approved, one layer
  per entry, and materialize from there; a local definition's path
  filesets are walked into guest-write specs at plan time; and inline
  filesets are lowered directly from the definition — all as launch-time
  snapshots. The trust story is that the files and the declaration that
  mounts them share one digest: a pulled document whose path entry no
  layer carries is refused rather than read off the consumer's disk. The
  absence of any signature warning on a fileset-carrying sandbox run is
  pinned end to end in the Layer 1 suite.

  Scenario: a pulled sandbox mounts its path fileset from the artifact it was approved at
    Given a published sandbox declaring a path fileset at "/root/.agent/skills"
    And its artifact carries 1 packed layer
    When the sandbox is planned
    Then the plan pulls the fileset at "/root/.agent/skills" from the sandbox's own artifact

  Scenario: a published sandbox whose artifact carries no layer for a path fileset is refused
    Given a published sandbox declaring a path fileset at "/root/.agent/skills"
    And its artifact carries 0 packed layers
    When the sandbox is planned
    Then the plan is refused naming the layer count

  Scenario: a mixin's packed fileset is pulled from the mixin's own artifact
    Given a published sandbox layering on a mixin that ships a path fileset at "/opt/skills"
    When the published sandbox is resolved and launched
    Then the run pulls "/opt/skills" from the mixin's own artifact

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

  Scenario: planning an inline fileset writes its exact content beneath the guest path
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

  Scenario: a published sandbox may carry inline files without any packed layer
    Given a published sandbox declaring an inline file at "/home/sandbox"
    When the sandbox is planned
    Then the plan accepts the inline fileset with nothing to pull

  Scenario: a hostPath fileset lands at its guestPath as a host-file write
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

  Scenario: a fileset landing under a volume target is staged so the mount cannot hide it
    Given a sandbox declaring inline file "tool.md" with content `read me` at "/home/node/.config"
    And the run mounts a writable named volume at "/home/node"
    When the sandbox is planned
    Then the plan stages the guest-write spec for "/home/node/.config/tool.md" for lns-init to copy in after the mount
    And the plan carries no guest-write spec for "/home/node/.config/tool.md"

  Scenario: a fileset landing outside every volume target is written straight into the rootfs
    Given a sandbox declaring inline file "tool.md" with content `read me` at "/etc/agent"
    And the run mounts a writable named volume at "/home/node"
    When the sandbox is planned
    Then the plan carries an inline guest-write spec for "/etc/agent/tool.md"
