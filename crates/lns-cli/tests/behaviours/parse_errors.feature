Feature: clap rejects bad input with exit code 2
  Operators who typo a subcommand, omit a required argument, or pass
  a non-parseable value get a clear clap error and a non-zero exit
  code — never a hang, a silent failure, or a fall-through to a
  default behavior.

  Scenario: lns run --cpus rejects a non-integer
    When I run "lns run --cpus abc"
    Then the exit code is 2
    And the output contains "invalid value"
    And the output contains "--cpus"

  Scenario: lns somebogus is rejected as an unknown subcommand
    When I run "lns somebogus"
    Then the exit code is 2
    And the output contains "unrecognized subcommand"

  Scenario: lns run -v with a non-absolute target is rejected before any service round-trip
    When I run "lns run -v data:notabsolute alpine"
    Then the exit code is 2
    And the output contains "must be an absolute path"

  Scenario: lns run -v with an invalid volume name is rejected
    When I run "lns run -v ../etc:/data alpine"
    Then the exit code is 2
    And the output contains "invalid volume name"

  Scenario: lns config set rejects an unknown key
    When I run "lns config set run.bogus 1"
    Then the exit code is 2
    And the output contains "unknown config key"

  Scenario: lns config set with no value reports the missing arg
    When I run "lns config set run.cpus"
    Then the exit code is 2
    And the output contains "required arguments were not provided"

  Scenario: lns audit --kind rejects an unknown event kind
    When I run "lns audit --kind bogus"
    Then the exit code is 2
    And the output contains "invalid value"
    And the output contains "--kind"
