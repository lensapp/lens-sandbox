# Out of scope for cargo test / regular CI: needs Vz/KVM and a booted guest.
# Lives under specs/microvm/ (not the globbed features/ dir), so it is inert
# documentation until a virt-capable runner is provisioned — then move it under
# the harness and select with `--tags @microvm`. See CLAUDE.md "Out of scope".
@microvm
Feature: published ports are reachable from the host through a real microVM
  This pins the live host→guest byte-path that the Layer 2 contract tests
  cannot exercise: the CLI grammar, the service-side forward, and the guest
  forwarder wired together against a booted guest serving a real port.

  Scenario: a published web port answers on the host loopback
    Given a microVM image that serves HTTP on guest port 3003
    When the user runs `lns run -d -p 3003:3003 <image>`
    And the workload reports its server is listening
    Then `curl http://127.0.0.1:3003/health` from the host returns 200

  Scenario: an unpublished port is not reachable from the host
    Given a microVM image that serves HTTP on guest port 3003
    When the user runs `lns run -d <image>` with no `-p`
    Then a host connection to 127.0.0.1:3003 is refused
