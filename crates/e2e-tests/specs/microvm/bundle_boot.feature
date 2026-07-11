# Parked under specs/microvm/ (not the globbed features/ dir): these scenarios
# exercised the full typed-artifact bundle path end to end, but their Given
# steps assembled and pushed the bundle with `lns build --push`, and the
# one-noun surface retired `lns build` (the flat `kind: Sandbox` artifact via
# `lns push` is the only CLI producer now). The AgentSystem wire contract is
# untouched — a published bundle still pulls, resolves its component graph,
# boots the sandbox baseImage, and runs the agent command — so these stay as
# spec with no step glue until the harness grows a test-only bundle producer
# (assembling and pushing the typed artifacts directly via oci_client to the
# in-process registry). The third scenario pins producer-side behaviour
# (auto-pinning floating component tags at publish); at run time a tag ref is
# fetched as-is, so it only means something once a producer exists again.
@microvm
Feature: a configured agent bundle boots from an OCI registry
  A producer assembles a Sandbox, an Agent, and a Bundle and pushes them to an
  in-process OCI registry on loopback (plaintext HTTP), then `lns run
  <bundle-ref>` pulls the bundle, resolves its component graph, boots the
  sandbox baseImage as the workload root, and runs the agent's own command.
  The baseImage is a real digest-pinned public image (alpine, pinned to the
  digest lns resolves), so like the other @microvm scenarios this reaches the
  network and runs only via `make e2e-microvm`.

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

  Scenario: publishing a bundle auto-pins its floating component tags before boot
    Given the Lens Sandbox service is running
    And a local registry holding a bundle whose components are referenced by tag
    When the user runs the bundle reference
    Then the exit code is 0
    And the output contains "bundle-boot-ok"
