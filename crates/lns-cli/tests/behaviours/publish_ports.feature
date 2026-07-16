Feature: lns run publishes guest ports to the host (docker `-p` grammar)
  Server-style workloads (e.g. a web UI on :3003) listen on a guest
  port, but the sandbox network is egress-only — nothing on the host can
  connect in. `lns run -p` publishes a guest port to the host using
  docker's `[host_ip:]hostport:containerport[/proto]` grammar. The one
  deliberate divergence from docker is the default host bind: loopback,
  not 0.0.0.0, so an untrusted workload is never silently exposed to the
  LAN. These scenarios pin the CLI grammar, the loopback default, and the
  run-summary surface against the in-process `Cli`; the live host→guest
  byte-path is a microVM concern, out of scope here. -p is the explicit
  side of publishing — how a definition's declared spec.ports become
  launch defaults is pinned in declarative_ports.feature.

  Scenario: Publishing a port maps host to guest with a loopback default
    Given the command is `lns run -p 3003:3003 prism`
    When the summary is printed
    Then a Ports line shows `127.0.0.1:3003 -> 3003`

  Scenario: An explicit host IP opts into wider exposure
    Given the command is `lns run -p 0.0.0.0:3003:3003 prism`
    When the summary is printed
    Then a Ports line shows `0.0.0.0:3003 -> 3003`
    And the run summary marks the mapping as exposed beyond this machine

  Scenario: Host and guest ports can differ
    Given the command is `lns run -p 8080:3003 prism`
    When the summary is printed
    Then a Ports line shows `127.0.0.1:8080 -> 3003`

  Scenario: Multiple ports can be published in one run
    Given the command is `lns run -p 3003:3003 -p 9090:9090 prism`
    When the summary is printed
    Then a Ports line shows `127.0.0.1:3003 -> 3003, 127.0.0.1:9090 -> 9090`

  Scenario: A udp suffix is parsed but rejected as not-yet-supported in v1
    When the user runs `lns run -p 5353:5353/udp prism`
    Then the command fails with an error that udp publishing is not yet supported
    And no run is started

  Scenario: A malformed -p spec is rejected before any run starts
    When the user runs `lns run -p notaport prism`
    Then the command fails with a parse error naming the bad spec
    And no run is started
