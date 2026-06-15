Feature: discovering and parsing the registry login subcommands
  `lns login` stores a verified credential for a private OCI registry so
  `lns run` and `lns image pull` can fetch its images; `lns logout` removes
  one. Both must be discoverable from help and parse cleanly, and `lns login`
  guards three mutually exclusive shapes: `--list` only reports, never logs in,
  and a secret comes from exactly one of `--password` or `--password-stdin`.
  The verify-then-store, canonicalization, and list-hides-secrets behaviour is
  pinned by Layer 3 unit tests, because this harness only parses argv — it
  never dispatches to the service or the auth store.

  Scenario: top-level help lists login and logout
    When I run "lns --help"
    Then the exit code is 0
    And the output contains "login"
    And the output contains "logout"

  Scenario: login --help describes the registry, credential, and list flags
    When I run "lns login --help"
    Then the exit code is 0
    And the output contains "Usage: lns login"
    And the output contains "OCI registry"
    And the output contains "--username"
    And the output contains "--password-stdin"
    And the output contains "--list"

  Scenario: logout --help describes removing a stored credential
    When I run "lns logout --help"
    Then the exit code is 0
    And the output contains "Usage: lns logout"
    And the output contains "Remove stored credentials"

  Scenario: --list cannot be combined with a username
    When I run "lns login --list --username me"
    Then the exit code is 2
    And the output contains "cannot be used with"
    And the output contains "--list"

  Scenario: --list cannot be combined with a piped password
    When I run "lns login --list --password-stdin"
    Then the exit code is 2
    And the output contains "cannot be used with"
    And the output contains "--list"

  Scenario: a password may not be given both inline and on stdin
    When I run "lns login --password-stdin --password secret --username me ghcr.io"
    Then the exit code is 2
    And the output contains "cannot be used with"
