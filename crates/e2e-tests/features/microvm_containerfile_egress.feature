@microvm
Feature: a build reaches only what the document's egress allows
  Slice 3 of lensapp/lens-sandbox#393. A `RUN` runs in a build guest under the
  document's own egress, so a fetch at build time is decided the way a fetch at run
  time is: the same rules, the same gate, the same refusal. A `RUN` the gate refuses
  fails the build, and the build stops at that instruction and names its line.

  Both scenarios write the rule into the mixin the project declares, because §8.5 of
  `docs/sandbox-spec.md` makes a rule apply only where a document names it. The first
  denies everything, so the destination is decided rather than asked about — an
  undecided destination raises a consent card, which no headless scenario can answer.
  The second allows one host and really reaches it, the way the tools feature really
  fetches from crates.io.

  Like every @microvm scenario this boots a real guest, so it runs only via
  `make e2e-microvm`, never in CI's PR gate.

  Scenario: a RUN that reaches a host the document denies fails the build at its line
    Given a clean lns cache home
    And the LNS service is running in that home
    And a network policy that denies all egress
    And the project builds its image from this Containerfile
      """
      FROM {base}
      RUN /.lens/guest-tools/bin/busybox wget -T 5 -O /dev/null http://example.com/
      """
    When the user runs the sandbox definition
    Then the exit code is non-zero
    And the output contains "example.com"
    And the output contains "line 2"

  Scenario: the same Containerfile with the host in egress builds and boots
    Given a clean lns cache home
    And the LNS service is running in that home
    And the document allows egress to "example.com"
    And the project builds its image from this Containerfile
      """
      FROM {base}
      RUN /bin/sh -c '/.lens/guest-tools/bin/busybox wget -T 20 -O /fetched http://example.com/ && echo fetched > /fetch-marker'
      """
    When the user runs a microVM command "/bin/sh -c 'cat /fetch-marker'"
    Then the exit code is 0
    And the output contains "fetched"
    And the run reports the image it built from "./image/Dockerfile"
