Feature: a declared port publishes, because declaring one is saying the sandbox serves on it
  A `spec.ports` entry is a statement that the sandbox serves on that
  port, so every run publishes it — the host: value when present, the
  container number when omitted — whether the document is one you wrote
  or one you pulled. There is no flag to opt in with: the run summary's
  Ports line discloses every binding before anything boots, and that
  summary is what you approve. The one deliberate divergence from docker
  stays: declared publishing binds loopback, never 0.0.0.0. Explicit -p
  entries union with the declared set and win on a container-port
  conflict.

  Scenario: running the local definition publishes its declared ports on loopback
    Given an lns.yaml declaring ports 3003 and 8080:9090
    When the local sandbox is run with no port flags
    Then a Ports line shows `127.0.0.1:3003 -> 3003, 127.0.0.1:8080 -> 9090`

  Scenario: a bare run of a pulled sandbox publishes its declared ports too
    Given a published sandbox declaring ports 3003 and 8080:9090
    When the sandbox reference is run with no port flags
    Then a Ports line shows `127.0.0.1:3003 -> 3003, 127.0.0.1:8080 -> 9090`

  Scenario: there is no flag to opt a pulled sandbox's declared ports in
    When I run "lns run -P ghcr.io/team/hermes:1.4.0"
    Then the exit code is 2
    And the output contains "unexpected argument"

  Scenario: an explicit -p wins over the declared mapping for the same container port
    Given an lns.yaml declaring ports 3003 and 9090
    When the local sandbox is run with `-p 4000:3003`
    Then a Ports line shows `127.0.0.1:4000 -> 3003, 127.0.0.1:9090 -> 9090`

  Scenario: explicit -p entries union with a pulled sandbox's declared ports
    Given a published sandbox declaring port 3003
    When the sandbox reference is run with `-p 5000:5000`
    Then a Ports line shows `127.0.0.1:3003 -> 3003, 127.0.0.1:5000 -> 5000`

  Scenario: two mappings on one host port are refused before boot
    Given an lns.yaml declaring ports 3003
    When the local sandbox is run with `-p 3003:9999` and the ports are composed
    Then composing the ports is refused, naming host port 3003

  Scenario: an explicit -p reusing a declared host port is refused
    Given an lns.yaml declaring ports 8080:9090
    When the local sandbox is run with `-p 8080:7000` and the ports are composed
    Then composing the ports is refused, naming host port 8080

  Scenario: the same mapping asked for twice is published once
    Given an lns.yaml declaring ports 3003
    When the local sandbox is run with `-p 4000:5000 -p 4000:5000` and the ports are composed
    Then `127.0.0.1:4000 -> 5000` is published exactly once
