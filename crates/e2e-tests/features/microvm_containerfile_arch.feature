@microvm
Feature: a second architecture adds an entry to the image index a document names
  Slice 6 of lensapp/lens-sandbox#393. A build runs on the architecture of the
  host that pushes, so a team on mixed hardware pushes one document from two
  hosts. The second push must add its entry to the image index the first
  published, not take the first's place (`docs/sandbox-spec.md` §6.2).

  The other architecture is written straight into the registry, under the tag a
  push from such a host would own, because this harness has only one
  architecture to build on. Everything after that is real: the push assembles the
  index over both entries, a home that has never built anything pulls the
  document, and the run boots the entry for the host it runs on.

  Like every @microvm scenario this boots real guests and reaches a real registry
  for the base image, so it runs only via `make e2e-microvm`, never in CI's PR gate.

  Scenario: a push from a second architecture adds its entry beside the first
    Given a clean lns cache home
    And a local registry
    And the LNS service is running in that home
    And the build context holds "app/index.js" containing "copied-by-lns"
    And the project builds its image from this Containerfile
      """
      FROM {base}
      RUN /bin/sh -c 'echo built-by-lns > /built-marker'
      COPY app /srv/app
      """
    When the user pushes the sandbox definition to the local registry
    Then the exit code is 0
    And the index holds only this host's architecture

    Given another architecture has pushed its image for the pushed sandbox
    When the user pushes the sandbox definition to the local registry
    Then the exit code is 0
    And the output contains "nothing to publish: the index already holds"
    And the index holds this host's architecture and the other one

    When the user pulls the pushed sandbox onto a machine that has never built it
    Then the exit code is 0

    When the user inspects the pushed sandbox
    Then the exit code is 0
    And inspect prints one built digest per architecture
    And the inspect booted no guest

    When the user runs the pushed sandbox with "/bin/sh -c 'cat /built-marker'"
    Then the exit code is 0
    And the output contains "built-by-lns"
    And the run booted this host's architecture
