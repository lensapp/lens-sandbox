Feature: machine-readable output from the list verbs
  Every list and status verb takes `--format json` so a script can consume it
  without parsing the human table. The JSON keys are camelCase, every key is
  present (null when there is no value), and byte counts stay raw integers so
  no consumer has to un-humanize "88 MiB".

  Scenario: ps emits one JSON object per running sandbox
    Given the service reports one running sandbox using 125 permille cpu and 92274688 bytes
    When the user runs sandbox command "ps --format json"
    Then the exit code is 0
    And the output is a JSON array of 1 rows
    And JSON row 0 has "name" set to "reviewer"
    And JSON row 0 has "image" set to "some-image"
    And JSON row 0 has "status" set to "running"
    And JSON row 0 has "memUsedBytes" set to 92274688
    And JSON row 0 has "cpuPermille" set to 125

  Scenario: ps json carries the full run id the table abbreviates
    Given the service reports one running sandbox using 125 permille cpu and 92274688 bytes
    When the user runs sandbox command "ps --format json"
    Then the exit code is 0
    And JSON row 0 has "id" set to "00000003000000000000000000000000"
    And the output does not contain "000000030000  "

  Scenario: ps with nothing running is an empty array, not absent output
    Given the service reports no runs
    When the user runs sandbox command "ps --format json"
    Then the exit code is 0
    And the output is an empty JSON array

  Scenario: the human table stays the default
    Given the service reports one running sandbox using 125 permille cpu and 92274688 bytes
    When the user runs sandbox command "ps"
    Then the exit code is 0
    And the output contains "CPU %"
    And the output contains "88.0 MiB"
