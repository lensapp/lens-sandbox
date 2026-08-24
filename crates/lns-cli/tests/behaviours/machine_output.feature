Feature: machine-readable output from the list verbs
  Every list and status verb takes `--format json` so a script can consume it
  without parsing the human table. The JSON keys are camelCase, every key is
  present (null when there is no value), and byte counts stay raw integers so
  no consumer has to un-humanize "88 MiB".

  Scenario: ps emits one JSON object per running sandbox
    Given the service reports one running sandbox using 125 permille cpu and 92274688 bytes
    When the user runs sandbox command "ls --format json"
    Then the exit code is 0
    And the output is a JSON array of 1 rows
    And JSON row 0 has "name" set to "reviewer"
    And JSON row 0 has "image" set to "some-image"
    And JSON row 0 has "status.state" set to "running"
    And JSON row 0 has "memUsedBytes" set to 92274688
    And JSON row 0 has "cpuPermille" set to 125

  Scenario: ps json carries the full run id the table abbreviates
    Given the service reports one running sandbox using 125 permille cpu and 92274688 bytes
    When the user runs sandbox command "ls --format json"
    Then the exit code is 0
    And JSON row 0 has "id" set to "00000003000000000000000000000000"
    And the output does not contain "000000030000  "

  Scenario: ps nulls the stats of a run whose guest stopped answering
    Given the service reports one running sandbox whose stats probe fails
    When the user runs sandbox command "ls --format json"
    Then the exit code is 0
    And the output is a JSON array of 1 rows
    And JSON row 0 has "name" set to "reviewer"
    And JSON row 0 has a null "cpuPermille"
    And JSON row 0 has a null "memUsedBytes"
    And JSON row 0 has a null "memTotalBytes"

  Scenario: the human table keeps a run whose guest stopped answering
    Given the service reports one running sandbox whose stats probe fails
    When the user runs sandbox command "ls"
    Then the exit code is 0
    And the output contains "reviewer"
    And the output does not contain "sampling guest stats failed"

  Scenario: ps with nothing running is an empty array, not absent output
    Given the service reports no runs
    When the user runs sandbox command "ls --format json"
    Then the exit code is 0
    And the output is an empty JSON array

  Scenario: the human table stays the default
    Given the service reports one running sandbox using 125 permille cpu and 92274688 bytes
    When the user runs sandbox command "ls"
    Then the exit code is 0
    And the output contains "CPU %"
    And the output contains "88.0 MiB"

  Scenario: artifact ls json exposes the fields the table has no room for
    Given the service reports one cached sandbox "hermes:1.4.0"
    When the user runs artifact command "ls --format json"
    Then the exit code is 0
    And the output is a JSON array of 1 rows
    And JSON row 0 has "reference" set to "hermes:1.4.0"
    And JSON row 0 has "sizeBytes" set to 14680064
    And JSON row 0 has "layers" set to 3
    And JSON row 0 has "digest" set to "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    And JSON row 0 has "pulled" set to "2026-01-01T00:00:00Z"
    And JSON row 0 has a null "inUseBy"

  Scenario: artifact ls with an empty cache is an empty array
    Given the service reports no cached sandboxes
    When the user runs artifact command "ls --format json"
    Then the exit code is 0
    And the output is an empty JSON array

  Scenario: volume ls json carries raw byte counts, not humanized sizes
    Given the service reports an idle volume "prism-data" using 33554432 bytes on disk
    When the user runs volume command "ls --format json"
    Then the exit code is 0
    And the output is a JSON array of 1 rows
    And JSON row 0 has "name" set to "prism-data"
    And JSON row 0 has "diskBytes" set to 33554432
    And JSON row 0 has a null "inUseBy"

  Scenario: volume ls with no volumes is an empty array
    Given the service reports no volumes
    When the user runs volume command "ls --format json"
    Then the exit code is 0
    And the output is an empty JSON array
