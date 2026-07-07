@microvm
Feature: a configured agent bundle boots from an OCI registry
  This exercises the full typed-artifact path end to end: `lns build --push`
  assembles a Sandbox, an Agent, and a Bundle and pushes them to an in-process
  OCI registry on loopback (plaintext HTTP), then `lns run <bundle-ref>` pulls
  the bundle, resolves its component graph, boots the sandbox baseImage as the
  workload root, and runs the agent's own command. The baseImage is a real
  digest-pinned public image (alpine, pinned to the digest lns resolves), so
  like the other @microvm scenarios this reaches the network and runs only via
  `make e2e-microvm`.

  Scenario: a bundle runs its agent command in the sandbox base image
    Given the Lens Sandbox service is running
    And a local registry holding a bundle whose agent runs "echo bundle-boot-ok"
    When the user runs the bundle reference
    Then the exit code is 0
    And the output contains "bundle-boot-ok"

  Scenario: a bundle's fileset is materialized into the sandbox before the agent runs
    Given the Lens Sandbox service is running
    And a local registry holding a bundle whose fileset the agent reads
    When the user runs the bundle reference
    Then the exit code is 0
    And the output contains "hello-from-fileset"
