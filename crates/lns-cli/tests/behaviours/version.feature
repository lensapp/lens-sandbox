Feature: lns --version
  Users running `lns --version` see the binary's name and version
  on stdout with a non-error exit code.

  Scenario: --version prints the lns version
    When I run "lns --version"
    Then the exit code is 0
    And the output contains "lns "
