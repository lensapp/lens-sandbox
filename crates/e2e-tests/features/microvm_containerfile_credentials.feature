@microvm
Feature: a build carries no credential the document did not declare
  Slice 3 of lensapp/lens-sandbox#393. A build step is the document's own run with one
  instruction in place of the workload, so what a `RUN` may read is what the document
  declares and nothing else: the service's own environment never reaches a guest, and a
  variable the machine that runs lns holds is not a variable the build can read.

  This is the negative half, asserted from inside the build guest by a `RUN` that writes
  what it can see of three variables into the image the run then boots: one the service
  process carries and the document never declared, one the document's `ENV` declares, and
  one its `ARG` declares. The positive half — a document that declares a credential a
  connector serves, and a build that starts with that connector's placeholder in the
  variable — needs an installed connector, which this harness has no step for yet; it is
  pinned at Layer 2 in lns-service.

  The verdict is written into the image rather than echoed by the boot command, because
  the run's own output repeats the command it was given: a phrase the command spells is a
  phrase the output holds whatever the guest saw.

  Like every @microvm scenario this boots a real guest, so it runs only via
  `make e2e-microvm`, never in CI's PR gate.

  Scenario: the environment a RUN sees holds the Containerfile's own values and nothing of the service's
    Given a clean lns cache home
    And the LNS service is running in that home carrying "LNS_E2E_HOST_ONLY=must-not-reach-a-build" in its own environment
    And the project builds its image from this Containerfile
      """
      FROM {base}
      ARG BUILD_ARG_VALUE=from-the-arg
      ENV AGENT_MODE=research
      RUN /bin/sh -c 'echo "host=[$LNS_E2E_HOST_ONLY]" > /verdict; echo "mode=[$AGENT_MODE]" >> /verdict; echo "arg=[$BUILD_ARG_VALUE]" >> /verdict'
      """
    When the user runs a microVM command "/bin/sh -c 'cat /verdict'"
    Then the exit code is 0
    And the output contains "host=[]"
    And the output contains "mode=[research]"
    And the output contains "arg=[from-the-arg]"
    And the output does not contain "must-not-reach-a-build"
    And the run reports the image it built from "./image/Dockerfile"
