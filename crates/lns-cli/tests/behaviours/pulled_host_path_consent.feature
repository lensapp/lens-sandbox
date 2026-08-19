Feature: a pulled sandbox's host file is decided per machine
  A hostPath fileset makes what a document mounts depend on the machine that
  runs it, so an artifact from a registry may not read one on the strength of
  its own declaration. The developer is asked on the first run that would read
  it, and the answer is recorded per machine — keyed by the artifact's
  repository and the host path, so a version bump does not ask again and a
  different sandbox does not inherit the answer. A document in the developer's
  own directory is their own consent and is never asked about.

  Background:
    Given no host path decision is recorded

  Scenario: the first run of a pulled sandbox asks about its host file
    Given a pulled sandbox "ghcr.io/team/hermes:1.4.0" reads host file "~/.gitconfig"
    And the user will answer "yes" to the host file prompt
    When the pulled sandbox host files are decided
    Then the prompt names "~/.gitconfig"
    And the prompt names "ghcr.io/team/hermes"
    And the run reads the host file
    And the recorded answer for "~/.gitconfig" is "allow"

  Scenario: the answer is recorded, so the next run does not ask
    Given a pulled sandbox "ghcr.io/team/hermes:1.4.0" reads host file "~/.gitconfig"
    And the user allowed "~/.gitconfig" for "ghcr.io/team/hermes"
    When the pulled sandbox host files are decided
    Then the developer is not asked
    And the run reads the host file

  Scenario: a version bump does not ask again
    Given a pulled sandbox "ghcr.io/team/hermes:2.0.0" reads host file "~/.gitconfig"
    And the user allowed "~/.gitconfig" for "ghcr.io/team/hermes"
    When the pulled sandbox host files are decided
    Then the developer is not asked
    And the run reads the host file

  Scenario: a different sandbox does not inherit the answer
    Given a pulled sandbox "ghcr.io/other/agent:1.0.0" reads host file "~/.gitconfig"
    And the user allowed "~/.gitconfig" for "ghcr.io/team/hermes"
    And the user will answer "yes" to the host file prompt
    When the pulled sandbox host files are decided
    Then the prompt names "~/.gitconfig"

  Scenario: declining an optional host file skips it and the run continues
    Given a pulled sandbox "ghcr.io/team/hermes:1.4.0" reads optional host file "~/.gitconfig"
    And the user will answer "no" to the host file prompt
    When the pulled sandbox host files are decided
    Then the run does not read the host file
    And the run continues
    And the recorded answer for "~/.gitconfig" is "deny"

  Scenario: declining a required host file refuses the run, and the refusal is remembered
    Given a pulled sandbox "ghcr.io/team/hermes:1.4.0" reads host file "~/.gitconfig"
    And the user will answer "no" to the host file prompt
    When the pulled sandbox host files are decided
    Then the run is refused naming "~/.gitconfig"
    And the recorded answer for "~/.gitconfig" is "deny"

  Scenario: an answer given before a refusal is kept
    Given a pulled sandbox "ghcr.io/team/hermes:1.4.0" reads optional host file "~/.vimrc" and required host file "~/.gitconfig"
    And the user will answer "yes" then "no" to the host file prompts
    When the pulled sandbox host files are decided
    Then the run is refused naming "~/.gitconfig"
    And the recorded answer for "~/.vimrc" is "allow"
    And the recorded answer for "~/.gitconfig" is "deny"

  Scenario: a decline is recorded too, so the next run does not ask
    Given a pulled sandbox "ghcr.io/team/hermes:1.4.0" reads optional host file "~/.gitconfig"
    And the user denied "~/.gitconfig" for "ghcr.io/team/hermes"
    When the pulled sandbox host files are decided
    Then the developer is not asked
    And the run does not read the host file

  Scenario: a pulled sandbox that ships only its own files asks nothing
    Given a pulled sandbox "ghcr.io/team/hermes:1.4.0" declares no host file
    And this machine's recorded answers cannot be read
    When the pulled sandbox host files are decided
    Then the developer is not asked
    And the run continues
    And nothing is recorded

  Scenario: a mixin's host file is decided even in your own directory
    Given a local sandbox whose mixin "ghcr.io/someone/toolkit@sha256:abc" reads host file "~/.gitconfig"
    And the user will answer "yes" to the host file prompt
    When the local sandbox host files are decided
    Then the prompt names "~/.gitconfig"
    And the prompt names "ghcr.io/someone/toolkit"
    And the run reads the host file

  Scenario: a mixin's host file is keyed to the mixin that declared it
    Given a local sandbox whose mixin "ghcr.io/someone/toolkit@sha256:abc" reads host file "~/.gitconfig"
    And the user allowed "~/.gitconfig" for "ghcr.io/someone/toolkit"
    When the local sandbox host files are decided
    Then the developer is not asked
    And the run reads the host file

  Scenario: declining a mixin's required host file refuses the local run
    Given a local sandbox whose mixin "ghcr.io/someone/toolkit@sha256:abc" reads host file "~/.gitconfig"
    And the user will answer "no" to the host file prompt
    When the local sandbox host files are decided
    Then the run is refused naming "~/.gitconfig"

  Scenario: a pulled sandbox's own host file is not keyed to a mixin it layers on
    Given a pulled sandbox "ghcr.io/team/hermes:1.4.0" reads host file "~/.gitconfig"
    And the user allowed "~/.gitconfig" for "ghcr.io/someone/toolkit"
    And the user will answer "yes" to the host file prompt
    When the pulled sandbox host files are decided
    Then the prompt names "ghcr.io/team/hermes"

  Scenario: a local definition's host file is never asked about
    Given a local sandbox reads host file "~/.gitconfig"
    When the local sandbox host files are decided
    Then the developer is not asked
    And the run reads the host file

  Scenario: a non-interactive run with no recorded answer fails closed
    Given a pulled sandbox "ghcr.io/team/hermes:1.4.0" reads optional host file "~/.gitconfig"
    And host file input is non-interactive
    When the pulled sandbox host files are decided
    Then the run is refused naming "--yes"
    And nothing is recorded

  Scenario: --yes accepts a host file without recording a decision
    Given a pulled sandbox "ghcr.io/team/hermes:1.4.0" reads host file "~/.gitconfig"
    And the user accepts every effect in advance
    When the pulled sandbox host files are decided
    Then the developer is not asked
    And the run reads the host file
    And nothing is recorded

  Scenario: --yes does not override an answer the developer already gave
    Given a pulled sandbox "ghcr.io/team/hermes:1.4.0" reads optional host file "~/.gitconfig"
    And the user denied "~/.gitconfig" for "ghcr.io/team/hermes"
    And the user accepts every effect in advance
    When the pulled sandbox host files are decided
    Then the developer is not asked
    And the run does not read the host file
