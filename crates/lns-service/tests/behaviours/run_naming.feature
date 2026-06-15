@serial
Feature: naming runs — the service owns names and resolution
  Every run carries a numeric id and a name. `--name` sets the name;
  omitting it auto-generates an adjective_noun one. Names are unique
  among listed runs, are addressable in place of the id, and are freed
  for reuse once their run is removed. `lns sandbox rename` changes a
  run's name in place.

  Scenario: a run registered without a name is auto-assigned an adjective_noun name
    Given a fresh service handler
    When a run is registered without a name
    Then the run is assigned a non-empty name
    And the assigned name is not all digits

  Scenario: a run registered with a name keeps that name
    Given a fresh service handler
    When a run is registered with the name "reviewer"
    Then the run's name is "reviewer"

  Scenario: a name already held by a listed run is rejected
    Given a registered run named "reviewer"
    When a run is registered with the name "reviewer"
    Then registration is refused
    And the refusal contains "already in use by run"

  Scenario: auto-generation regenerates until the name is unique among listed runs
    Given a registered run whose name is the generator's first pick
    When a run is registered without a name
    Then the auto-assigned name differs from every listed run's name

  Scenario: a name resolves to its run, as does the numeric id
    Given a registered run named "reviewer" that has already exited
    When a StopRun request for run "reviewer" arrives
    Then the response is RunStopped without force
    When a StopRun request for that run's numeric id arrives
    Then the response is RunStopped without force

  Scenario: an unknown name surfaces a "no such run" error
    Given a fresh service handler
    When a StopRun request for run "ghost" arrives
    Then the response is Error
    And the error message contains "no such run: ghost"

  Scenario: a name frees up once its run is removed and can be reused
    Given a registered run named "reviewer" that has already exited
    When a RemoveRun request for run "reviewer" arrives
    Then the response is Acknowledged
    And a run can then be registered with the name "reviewer"

  Scenario: an all-digit name is rejected
    Given a fresh service handler
    When a run is registered with the name "7"
    Then registration is refused
    And the refusal explains a name must not be all digits

  Scenario: a name with an illegal character is rejected
    Given a fresh service handler
    When a run is registered with the name "has space"
    Then registration is refused

  Scenario: rename changes the name in place
    Given a registered run named "reviewer"
    When a RenameRun request renames "reviewer" to "auditor"
    Then the response is Acknowledged
    And the run resolves by the name "auditor"
    And the run no longer resolves by the name "reviewer"

  Scenario: rename to a held name is refused
    Given a registered run named "reviewer"
    And a registered run named "auditor"
    When a RenameRun request renames "auditor" to "reviewer"
    Then the response is Error
    And the error message contains "already in use"

  Scenario: rename to an all-digit name is refused
    Given a registered run named "reviewer"
    When a RenameRun request renames "reviewer" to "7"
    Then the response is Error
