Feature: lns run --env-file loads workload environment variables from a file
  `lns run --env-file FILE` injects non-secret configuration in bulk,
  mirroring `docker run --env-file`. Files merge in flag order, `-e`
  always wins over file entries, and malformed lines fail before any
  service round-trip. Bare `KEY` lines (host passthrough) are refused
  for the same reason `-e KEY` is: they would be a silent
  host-to-workload leak channel.

  Scenario: Variables from an env file reach the run environment
    Given the working directory contains an env file `app.env`:
      """
      PORT=3003
      NODE_ENV=production
      """
    When the run environment is assembled for `lns run --env-file app.env someimage`
    Then the run environment contains `PORT=3003`
    And the run environment contains `NODE_ENV=production`

  Scenario: -e overrides the same variable from an env file
    Given the working directory contains an env file `app.env`:
      """
      PORT=3003
      """
    When the run environment is assembled for `lns run --env-file app.env -e PORT=4000 someimage`
    Then the run environment contains `PORT=4000`
    And the run environment does not contain `PORT=3003`

  Scenario: A later env file overrides an earlier one
    Given the working directory contains an env file `base.env`:
      """
      LOG_LEVEL=info
      PORT=3003
      """
    And the working directory contains an env file `override.env`:
      """
      LOG_LEVEL=debug
      """
    When the run environment is assembled for `lns run --env-file base.env --env-file override.env someimage`
    Then the run environment contains `LOG_LEVEL=debug`
    And the run environment contains `PORT=3003`
    And the run environment does not contain `LOG_LEVEL=info`

  Scenario: Comment and blank lines are skipped
    Given the working directory contains an env file `app.env`:
      """
      # tuning knobs

      PORT=3003
      """
    When the run environment is assembled for `lns run --env-file app.env someimage`
    Then the run environment contains `PORT=3003`

  Scenario: A bare KEY line is refused (no host passthrough)
    Given the working directory contains an env file `app.env`:
      """
      PORT=3003
      HOME
      """
    When the run environment is assembled for `lns run --env-file app.env someimage`
    Then the merge fails naming `app.env` line 2
    And the merge failure requires KEY=VALUE form

  Scenario: A missing env file fails before any run starts
    When the run environment is assembled for `lns run --env-file nope.env someimage`
    Then the merge fails with an error mentioning `nope.env`
