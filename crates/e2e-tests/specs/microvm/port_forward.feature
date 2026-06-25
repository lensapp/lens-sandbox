# Parked under specs/microvm/ (not the globbed features/ dir): these scenarios
# need a guest that serves a TCP port, which an imageless run cannot provide —
# the bundled busybox ships no server applet (no httpd/nc/telnetd/…), confirmed
# by inspecting the cached binary. So, like volume_seeding.feature, they need a
# real image with a server, blocked on a local-image path (`lns image import`).
# The CLI grammar, service-side forward, and connection-refused path are covered
# by Layer 2 contract tests; this file is the live host->guest byte-path, inert
# until an image source exists. See CLAUDE.md "Out of scope".
@microvm
Feature: published ports are reachable from the host through a real microVM

  Scenario: a published web port answers on the host loopback
    Given a microVM image that serves HTTP on guest port 3003
    When the user runs `lns run -d -p 3003:3003 <image>`
    And the workload reports its server is listening
    Then `curl http://127.0.0.1:3003/health` from the host returns 200

  Scenario: an unpublished port is not reachable from the host
    Given a microVM image that serves HTTP on guest port 3003
    When the user runs `lns run -d <image>` with no `-p`
    Then a host connection to 127.0.0.1:3003 is refused
