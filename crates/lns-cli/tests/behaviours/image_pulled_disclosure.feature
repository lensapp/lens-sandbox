Feature: what a pulled artifact discloses about the image it was built from
  A document published from a Containerfile carries that file and its context
  as a layer of the same artifact (`docs/sandbox-spec.md` §7.3), so `lns
  inspect` on a pulled artifact prints the instructions and lists the context
  with sizes, then the digest the guest actually starts from — without
  building or running anything.

  Scenario: inspect prints the Containerfile, the context and the built digest
    Given the service inspects "ghcr.io/team/hermes:1.4.0" as a sandbox built from "./image/Containerfile"
    When the user runs artifact command "inspect ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "image: ghcr.io/team/hermes@sha256:"
    And the output contains "imageSource: built from ./image/Containerfile (2 lines, context 2 files)"
    And the output contains "context: app/main.js (15 B)"
    And the output contains "RUN npm install -g @anthropic-ai/claude-code"

  Scenario: a sandbox whose image was pulled rather than built discloses no source
    Given the service inspects "ghcr.io/team/hermes:1.4.0" as a sandbox with launch settings
    When the user runs artifact command "inspect ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output does not contain "imageSource"
