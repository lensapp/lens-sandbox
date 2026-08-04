@microvm
Feature: every workload guest can verify a TLS peer
  The supervisor appends the proxy CA to the guest's system trust store, and
  it can only append to a store that already exists. A guest whose rootfs
  ships none used to end up with no roots at all — a failure no host-side
  test can see, because it depends on what the booted rootfs carries. The
  service now stages the pinned public bundle on every run and the broker
  seeds it before any session forks. These scenarios boot a real guest to pin
  both halves: a rootfs without a store gets the pinned roots, and a rootfs
  with its own keeps every root it added.

  Scenario: a rootfs that carries no trust store gets the pinned public roots
    Given the Lens Sandbox service is running
    And a base image that ships no trust store
    When the user runs a microVM command "/bin/sh -c 'grep -m1 BEGIN.CERTIFICATE /etc/ssl/certs/ca-certificates.crt'"
    Then the exit code is 0
    And the output contains "BEGIN CERTIFICATE"

  Scenario: a rootfs that carries its own trust store keeps it
    Given the Lens Sandbox service is running
    And a base image that ships no trust store
    And the project declares inline file "ca-certificates.crt" with content `e2e-image-root` mounted at "/etc/ssl/certs"
    When the user runs a microVM command "/bin/sh -c 'cat /etc/ssl/certs/ca-certificates.crt'"
    Then the exit code is 0
    And the output contains "e2e-image-root"
