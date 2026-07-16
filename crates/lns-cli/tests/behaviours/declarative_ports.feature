@todo
Feature: declared ports publish like compose locally and like docker run when pulled
  spec.ports follows the docker family on both sides of the publish
  decision. Running the definition in your own directory is `docker
  compose up`: every declared port publishes automatically — the host:
  value when present, the container number when omitted — because the
  definition you wrote is your consent, and the run summary's Ports line
  discloses each binding. Running a pulled sandbox is `docker run`: its
  declared ports are EXPOSE-style declarations only, published only when
  the consumer opts in with -P/--publish-declared. The one deliberate
  divergence from docker stays: declared publishing binds loopback, never
  0.0.0.0. Explicit -p entries union with the declared set and win on a
  container-port conflict.

  Scenario: running the local definition publishes its declared ports on loopback
    Given an lns.yaml declaring ports 3003 and 8080:9090
    When the local sandbox is run with no port flags
    Then a Ports line shows `127.0.0.1:3003 -> 3003, 127.0.0.1:8080 -> 9090`

  Scenario: a bare run of a pulled sandbox publishes nothing
    Given a published sandbox declaring port 3003
    When the sandbox reference is run with no port flags
    Then a Ports line shows `(none)`
    And the summary notes port 3003 is declared but not published

  Scenario: -P publishes a pulled sandbox's declared ports on loopback
    Given a published sandbox declaring ports 3003 and 8080:9090
    When the sandbox reference is run with `-P`
    Then a Ports line shows `127.0.0.1:3003 -> 3003, 127.0.0.1:8080 -> 9090`

  Scenario: an explicit -p wins over the declared mapping for the same container port
    Given an lns.yaml declaring ports 3003 and 9090
    When the local sandbox is run with `-p 4000:3003`
    Then a Ports line shows `127.0.0.1:4000 -> 3003, 127.0.0.1:9090 -> 9090`

  Scenario: explicit -p entries union with -P on a pulled sandbox
    Given a published sandbox declaring port 3003
    When the sandbox reference is run with `-P -p 5000:5000`
    Then a Ports line shows `127.0.0.1:3003 -> 3003, 127.0.0.1:5000 -> 5000`

  Scenario: -P on the local definition is accepted and redundant
    Given an lns.yaml declaring ports 3003
    When the local sandbox is run with `-P`
    Then a Ports line shows `127.0.0.1:3003 -> 3003`
