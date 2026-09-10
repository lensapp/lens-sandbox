Feature: what a pulled artifact discloses about the image it was built from
  A document published from a Containerfile carries that file and its context
  as a layer of the same artifact (`docs/sandbox-spec.md` §7.3), so `lns
  inspect` on a pulled artifact prints the instructions and lists the context
  with sizes, then one built digest per architecture the published index holds
  (§6) — without building or running anything.

  Scenario: inspect prints the Containerfile, the context and one digest per architecture
    Given the service inspects "ghcr.io/team/hermes:1.4.0" as a sandbox built for arm64 and amd64 from "./image/Containerfile"
    When the user runs artifact command "inspect ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "image: built from ./image/Containerfile (2 lines, context 2 files), arm64 sha256:aaaa, amd64 sha256:bbbb"
    And the output contains "context: app/main.js (15 B)"
    And the output contains "RUN npm install -g @anthropic-ai/claude-code"
    And the output prints "RUN npm install -g @anthropic-ai/claude-code" after "mount: bind . -> /workspace"

  Scenario: a sandbox whose image was pulled rather than built discloses no source
    Given the service inspects "ghcr.io/team/hermes:1.4.0" as a sandbox with launch settings
    When the user runs artifact command "inspect ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output does not contain "built from"

  Scenario: an artifact whose index this machine cannot read still names the image it runs
    Given the service inspects "ghcr.io/team/hermes:1.4.0" as a sandbox built from "./image/Containerfile"
    When the user runs artifact command "inspect ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "image: built from ./image/Containerfile (2 lines, context 2 files), ghcr.io/team/hermes@sha256:"
