Feature: listing the registries you are logged in to
  `lns login --list` answers with data, so it is a list verb like any
  other: a table with an uppercase header row by default, `--format json`
  for a script, and one sentence when there is nothing to list. It reports
  hosts and usernames only — a stored secret never leaves the service.

  Scenario: the list is a table with an uppercase header row
    Given the service reports a login to "ghcr.io" as "octocat"
    When I log in with "lns login --list"
    Then the exit code is 0
    And the output contains "REGISTRY"
    And the output contains "USERNAME"
    And the output contains "ghcr.io"
    And the output contains "octocat"

  Scenario: --format json answers with an array a script can read
    Given the service reports a login to "ghcr.io" as "octocat"
    When I log in with "lns login --list --format json"
    Then the exit code is 0
    And the output is a JSON array of 1 rows
    And JSON row 0 has "registry" set to "ghcr.io"
    And JSON row 0 has "username" set to "octocat"

  Scenario: with no logins, the table says so instead of printing a bare header
    Given the service reports no registry logins
    When I log in with "lns login --list"
    Then the exit code is 0
    And the output contains "Not logged in to any registry."
    And the output does not contain "REGISTRY"

  Scenario: with no logins, json stays an empty array
    Given the service reports no registry logins
    When I log in with "lns login --list --format json"
    Then the exit code is 0
    And the output is an empty JSON array
