Feature: lns config stores persistent run defaults
  `lns config` records set-once defaults (vCPUs, memory, env vars,
  volumes, published ports) in a per-user config file so common
  settings don't need a flag on every `lns run`. Values are validated
  when stored with the same parsers the run flags use, so a typo
  surfaces immediately instead of breaking a future run.

  Scenario: Setting a default records it in the config file
    When the developer sets the default "run.cpus" to "4"
    Then the command reports the default was set
    And getting "run.cpus" prints "4"

  Scenario: Getting a default that is not set exits 1 with no output
    When the developer gets the default "run.cpus"
    Then the command exits 1 with no output

  Scenario: Listing shows every configured default
    Given the default "run.cpus" is "4"
    And the default "run.registry" is "ghcr.io"
    When the developer lists the configured defaults
    Then the listing shows "run.cpus = 4"
    And the listing shows "run.registry = ghcr.io"

  Scenario: Listing with nothing configured says so
    When the developer lists the configured defaults
    Then the output says no defaults are configured

  Scenario: Unsetting a default removes it
    Given the default "run.cpus" is "4"
    When the developer unsets the default "run.cpus"
    Then getting "run.cpus" exits 1 with no output

  Scenario: Unsetting a default that is not set fails
    When the developer unsets the default "run.mem"
    Then the command fails with an exit code other than 0

  Scenario: A single-value key rejects multiple values
    When the developer sets the default "run.cpus" to "4 8"
    Then the command fails mentioning "a single value"

  Scenario: A non-numeric cpu default is rejected when set
    When the developer sets the default "run.cpus" to "many"
    Then the command fails mentioning "run.cpus"

  Scenario: A memory default may use a unit suffix and is stored in MiB
    When the developer sets the default "run.mem" to "2g"
    Then getting "run.mem" prints "2048"

  Scenario: A zero cpu default is rejected when set
    When the developer sets the default "run.cpus" to "0"
    Then the command fails mentioning "at least 1"

  Scenario: A zero memory default is rejected when set
    When the developer sets the default "run.mem" to "0"
    Then the command fails mentioning "run.mem"

  Scenario: Listing the defaults as JSON yields key/value rows for scripts
    Given the default "run.cpus" is "4"
    And the default "run.registry" is "ghcr.io"
    When the developer lists the configured defaults as JSON
    Then the exit code is 0
    And the output is a JSON array of 2 rows
    And JSON row 0 has "key" set to "run.cpus"
    And JSON row 0 has "value" set to "4"

  Scenario: Listing nothing configured as JSON is an empty array, not prose
    When the developer lists the configured defaults as JSON
    Then the exit code is 0
    And the output is an empty JSON array

  Scenario: Getting a set default as JSON gives one object, not a list of one
    Given the default "run.cpus" is "4"
    When the developer gets the default "run.cpus" as JSON
    Then the exit code is 0
    And the output is a JSON object
    And the JSON object has "key" set to "run.cpus"
    And the JSON object has "value" set to "4"

  Scenario: Getting an unset default as JSON still exits 1, answering null
    When the developer gets the default "run.cpus" as JSON
    Then the exit code is 1
    And the output is JSON null
