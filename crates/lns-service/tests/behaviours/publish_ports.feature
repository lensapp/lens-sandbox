Feature: lns-service forwards published ports over vsock
  When a run carries published port mappings, the service opens a
  host-side listener for each one and bridges accepted connections over
  the existing host↔guest vsock channel to a guest-side forwarder that
  dials the container port. These scenarios pin the service-side contract
  and lifecycle against the in-process dispatcher with the VM / network
  ports mocked — the actual byte-path needs a booted microVM and is
  covered elsewhere. The egress proxy/DNS cage is unaffected: publishing
  only adds an inbound path for ports the user explicitly named.

  Scenario: The service requests a host-side forward for each published port
    Given a run is started with published mapping 127.0.0.1:3003 -> 3003
    When the service sets up the run
    Then it requests a host listener on 127.0.0.1:3003 forwarding to guest 3003

  Scenario: A host port already in use fails the run fast with a clear error
    Given host port 3003 is already bound
    And a run is started with published mapping 127.0.0.1:3003 -> 3003
    When the service sets up the run
    Then the run fails before boot with an "address already in use" error
    And the process exits non-zero

  Scenario: Published host ports are released when the run ends
    Given a run published 127.0.0.1:3003 -> 3003
    When the run exits
    Then the host port 3003 is freed for reuse

  Scenario: A detached run keeps its published ports up until the run is killed or exits
    Given a detached run published 127.0.0.1:3003 -> 3003
    When the CLI detaches
    Then the host listener on 127.0.0.1:3003 stays up
    And it is torn down only when the run is killed or exits

  Scenario: Only published ports are reachable; the egress model is unchanged
    Given a run published only 127.0.0.1:3003 -> 3003
    Then no other guest port is reachable from the host
    And outbound network still flows through the existing proxy/DNS cage
