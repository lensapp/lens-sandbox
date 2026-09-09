@microvm
Feature: a build carries no credential the document did not declare
  Slice 3 of lensapp/lens-sandbox#393. A build step is the document's own run with one
  instruction in place of the workload, so what a `RUN` may read is what the document
  declares and nothing else: no connector this machine installed reaches a build whose
  document names none, and the service's own environment never reaches a guest.

  This is the negative half, asserted from inside the build guest by a `RUN` that writes
  its whole environment into the image the run then boots. The positive half — a
  document that declares a credential a connector serves, and a build that starts with
  that connector's placeholder in the variable — needs an installed connector, and the
  @microvm harness has no step for one yet; it is pinned at Layer 2 in lns-service.

  Like every @microvm scenario this boots a real guest, so it runs only via
  `make e2e-microvm`, never in CI's PR gate.

  Scenario: the environment a RUN sees holds the Containerfile's own values and no credential
    Given a clean lns cache home
    And the LNS service is running in that home
    And the project builds its image from this Containerfile
      """
      FROM {base}
      ARG BUILD_ARG_VALUE=from-the-arg
      ENV AGENT_MODE=research
      RUN /bin/sh -c 'env > /build-env; echo arg=$BUILD_ARG_VALUE >> /build-env'
      """
    When the user runs a microVM command "/bin/sh -c 'if /.lens/guest-tools/bin/busybox grep -q LNSPLACEHOLDER /build-env; then echo credential=leaked; else echo credential=none; fi; /.lens/guest-tools/bin/busybox grep AGENT_MODE=research /build-env; /.lens/guest-tools/bin/busybox grep arg=from-the-arg /build-env'"
    Then the exit code is 0
    And the output contains "credential=none"
    And the output contains "arg=from-the-arg"
    And the output contains "AGENT_MODE=research"
    And the output does not contain "credential=leaked"
    And the run reports the image it built from "./image/Dockerfile"
