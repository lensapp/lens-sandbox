Feature: a run the service refuses never starts
  §3.3.2 has a resolution refuse when two sources publish one host port, and
  §3.1.11 has a host port already in use refuse the run. A refusal is not a
  failure of a run that started: nothing was booted, nothing was recorded, and
  the developer asked for something the service can answer for immediately.

  So the answer arrives before the run does. A run start decides everything it
  can be turned away for while it is still only a request — its host ports, what
  its reference resolves to, and whether the run can be identified at all — and
  only then takes a place in the registry and tells its client the run id. A
  refusal costs no run id, no run name, and no started run to clean up.

  Scenario: a resolution that refuses answers before the run starts
    Given a run whose sources publish one host port twice
    When the run request arrives
    Then the service answers with the refusal, naming the port
    And the client is never told a run started
    And no run is registered

  Scenario: a run nothing refuses reaches the step that registers it
    Given a run nothing refuses
    When the run request arrives
    Then the run is served

  Scenario: a run asking for a name another run holds is refused before it starts
    Given a registered run named "held-by-another"
    And a run nothing refuses
    When the run request asks for the name "held-by-another"
    Then the service answers that the name is in use
    And the client is never told a run started
    And the run is not served
