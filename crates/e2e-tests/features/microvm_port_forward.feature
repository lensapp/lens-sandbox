@microvm
Feature: only published ports are reachable from the host through a real microVM
  This pins the live host->guest byte path that the Layer 2 contract tests
  cannot exercise: the CLI grammar, the service-side forward, and the guest
  forwarder wired together against a booted guest. It is imageless — the guest
  serves with the bundled busybox `nc` and the host reads the bytes back over a
  raw TCP connection (no server image, no HTTP parser needed). One run listens
  on two ports but publishes only one, so the negative probe targets a port the
  guest is genuinely serving yet was never exposed — a regression that forwarded
  everything, or a dead guest, both turn it red. The host ports are uncommon to
  reduce the chance of colliding with a service already running on the machine.

  Scenario: a published port is reachable but an unpublished one the guest serves is not
    Given the Lens Sandbox service is running
    When the user starts a detached microVM command "/bin/sh -c '(while true; do printf published | /.lens/guest-tools/bin/busybox nc -l -p 47821; done) & while true; do printf private | /.lens/guest-tools/bin/busybox nc -l -p 47822; done'" publishing port 47821
    Then the exit code is 0
    And the host can fetch "published" from port 47821
    And the host cannot connect to port 47822
