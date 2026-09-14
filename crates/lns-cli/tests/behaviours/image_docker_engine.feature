Feature: a machine that builds on its own Docker daemon says so
  `build.engine` may point a Containerfile build at the host Docker daemon
  instead of a build guest (`docs/sandbox-spec.md` §3.1.1). The document's
  egress and credentials decide nothing there, so the entry the push publishes
  inside the image index carries the record and `lns inspect` reads it back
  (§6.2). With the switch off nothing changes.

  Background:
    Given an lns.yaml whose image is "./image"
    And the file "image/Containerfile" holds:
      """
      FROM docker.io/library/node:24-bookworm
      RUN npm install -g @anthropic-ai/claude-code
      """
    And the file "image/app/main.js" holds:
      """
      console.log(1)
      """

  Scenario: a push of an image the host daemon built records it on the index entry and says so
    Given the registry accepts the push
    And this machine builds on the host Docker daemon
    And the build answers with an image of 2 layers
    When the user runs artifact command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the image was published into "ghcr.io/team/hermes"
    And the index entry for "arm64" says it was built outside the gate
    And the output contains "built outside the gate by the host Docker daemon"
    And the build the push asked for names the host Docker daemon

  Scenario: the same document with the switch off publishes an entry that says nothing about the gate
    Given the registry accepts the push
    And the build answers with an image of 2 layers
    When the user runs artifact command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the index entry for "arm64" says nothing about the gate
    And the output does not contain "built outside the gate by the host Docker daemon"

  Scenario: a sandbox build sends the switch this machine is set to
    Given a document whose spec.image names a Containerfile
    And this machine builds on the host Docker daemon
    And the service builds it into 3 layers
    When the user runs sandbox command "build"
    Then the exit code is 0
    And the build request names the host Docker daemon

  Scenario: a sandbox build with the switch off asks for a build guest
    Given a document whose spec.image names a Containerfile
    And the service builds it into 3 layers
    When the user runs sandbox command "build"
    Then the exit code is 0
    And the build request names a build guest

  Scenario: inspect marks the architecture a daemon built and leaves the other alone
    Given the service inspects "ghcr.io/team/hermes:1.4.0" as a sandbox built for arm64 and amd64 from "./image/Containerfile"
    And the index says "amd64" was built outside the gate
    When the user runs artifact command "inspect ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "arm64 sha256:aaaa, amd64 sha256:bbbb (amd64 was built outside the gate by the host Docker daemon)"
