Feature: a client that leaves takes its unstarted run with it
  A run start decides its refusals before the run exists, and deciding them can
  reach the network — a manifest peek, a mixin pull. A registry that never
  answers would otherwise leave that work with nobody to stop it: the run has no
  registry entry to cancel, and the host ports it bound stay bound.

  So the run start watches the client it is answering. The client sends nothing
  between its request and the run id, so anything the service reads there is the
  client going away, and a run nobody is waiting for stops being prepared.

  Scenario: a client that disconnects while its run is prepared is not kept waiting on
    Given a run whose preparation never finishes
    When the client goes away before the run starts
    Then the run start gives up and serves nothing

  Scenario: a client that sends something before its run starts does not stop it
    Given a run nothing refuses
    When the client sends a stray byte before the run starts
    Then the run is served
