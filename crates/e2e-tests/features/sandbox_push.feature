Feature: publishing a sandbox in one step
  `lns push <ref>` reads `./lns.yaml`, builds the sandbox artifact, and uploads
  it to the registry in one step — no separate build cache. This runs virt-free
  (no microVM, no daemon) against an in-process registry on loopback, so it is
  part of the regular e2e suite.

  Scenario: push builds ./lns.yaml and uploads it
    Given a local registry
    When the user pushes a sandbox built from ./lns.yaml in one step
    Then the registry serves the pushed artifact at its ref
