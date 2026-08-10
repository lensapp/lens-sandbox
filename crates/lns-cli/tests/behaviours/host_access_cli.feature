Feature: lns host-access lists, grants, and revokes host capabilities
  A directory grants a host capability by recording its id in `lns-policy.yaml`,
  which is how a directory with no sandbox definition arms one and how a run
  stops raising the launch card. The grant list is shareable — it names ids, not
  host paths — while the per-machine decline lives outside it. Granting therefore
  also clears a standing decline, because the remembered no must not beat the yes
  the operator just typed.

  Scenario: list marks which capabilities this directory has granted
    Given this directory grants host access "git-signing"
    When the user runs host-access command "list"
    Then the exit code is 0
    And the host-access output contains "git-signing"
    And the host-access output contains "granted"

  Scenario: list shows an ungranted capability as available
    When the user runs host-access command "list"
    Then the exit code is 0
    And the host-access output contains "available"

  Scenario: grant records the id in this directory's policy
    When the user runs host-access command "grant git-signing"
    Then the exit code is 0
    And this directory's policy grants "git-signing"
    And the host-access output contains "Granted git-signing"

  Scenario: granting twice is idempotent and says so
    Given this directory grants host access "git-signing"
    When the user runs host-access command "grant git-signing"
    Then the exit code is 0
    And the host-access output contains "already granted"
    And this directory's policy grants "git-signing"

  Scenario: granting refuses an id this machine cannot resolve
    When the user runs host-access command "grant not-in-catalog"
    Then the exit code is 1
    And the host-access output contains "unknown host access"
    And this directory's policy grants nothing

  Scenario: granting clears a standing decline so the yes wins
    Given a standing decline is recorded for host access "git-signing"
    When the user runs host-access command "grant git-signing"
    Then the exit code is 0
    And the host-access output contains "standing decline on this machine is cleared"
    And no standing decline remains for host access "git-signing"

  Scenario: revoke removes the grant
    Given this directory grants host access "git-signing"
    When the user runs host-access command "revoke git-signing"
    Then the exit code is 0
    And this directory's policy grants nothing
    And the host-access output contains "Revoked git-signing"

  Scenario: revoking what was never granted reports it and exits non-zero
    When the user runs host-access command "revoke git-signing"
    Then the exit code is 1
    And the host-access output contains "is not granted"
