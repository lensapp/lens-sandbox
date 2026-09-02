Feature: the working directory is read, never written

  `sandbox-spec.md` §8.5: the working directory roots the paths the user types
  and nothing else. `lns` creates no file there that the user did not name, and
  `save` — the one verb that writes one — never destroys a file already there.

  Scenario: a run creates no file in the directory it was started from
    Given a clean lns cache home
    And a working directory holding "lns.yaml"
    When I run "lns run" in that directory
    Then the command fails with an exit code other than 0
    And the working directory holds only "lns.yaml"


  Scenario: save with no file to write to is refused by name
    Given a clean lns cache home
    When I run "lns sandbox save some-run"
    Then the exit code is 2
    And the output contains "--file"

  Scenario: a save that cannot complete leaves the named file as it was
    Given a clean lns cache home
    And a working directory holding "keep.yaml"
    When I run "lns sandbox save some-run -f keep.yaml" in that directory
    Then the command fails with an exit code other than 0
    And "keep.yaml" still holds what it held
