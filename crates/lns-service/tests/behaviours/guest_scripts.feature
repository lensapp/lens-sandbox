Feature: a run's pre-start scripts are staged for the guest to run
  A sandbox or a mixin may declare spec.scripts: shell scripts the guest
  runs before the workload starts, each under a user it names. The
  service stages each one as a file inside the run's own runtime layer
  plus one manifest naming them in run order, because a multi-line
  script cannot ride the kernel cmdline and the session channel's schema
  is not ours to extend. Staging is what carries the ordering decision
  across the host/guest boundary: mixin-contributed scripts append to
  the sandbox's own rather than overriding them, so the manifest's order
  is the merge's order. The staged files are root-owned and unwritable
  by the workload, so a workload cannot rewrite the script that a later
  run of the same sandbox will execute. What the supervisor then does
  with the manifest is its own concern, pinned in its own suite.

  Scenario: a sandbox's scripts are staged in declaration order
    Given a local definition declaring the pre-start scripts "npm ci" and "npm run build"
    When the run is planned
    Then the run stages 2 scripts
    And the staged scripts run in the order "npm ci", "npm run build"

  Scenario: a mixin's scripts are staged after the sandbox's own
    Given a local definition declaring the pre-start script "npm ci"
    And it layers on a mixin declaring the pre-start script "apt-get install -y psql"
    When the run is planned
    Then the staged scripts run in the order "npm ci", "apt-get install -y psql"

  Scenario: a script naming root travels with that user beside its body
    Given a local definition declaring a pre-start script "apt-get install -y psql" as "root"
    When the run is planned
    Then the staged script for "apt-get install -y psql" names the user "root"

  Scenario: a script naming no user defers to the run's own run-as identity
    Given a local definition declaring the pre-start script "npm ci"
    When the run is planned
    Then the staged script for "npm ci" names no user

  Scenario: a script is staged where the guest reads it, root-owned and unwritable
    Given a local definition declaring the pre-start script "npm ci"
    When the run is planned
    Then every staged script sits beneath "/.lens/scripts" and is not writable

  Scenario: a run declaring no scripts stages no manifest at all
    Given a local definition declaring no pre-start scripts
    When the run is planned
    Then the run stages no script manifest

  Scenario: a script's own description is what the manifest labels it by
    Given a local definition declaring a pre-start script "apt-get install -y psql" described as "the psql the prompts assume"
    When the run is planned
    Then the staged script for "apt-get install -y psql" is labelled "the psql the prompts assume"

  Scenario: a script with no description is labelled by its first line
    Given a local definition declaring the pre-start script "apt-get update\napt-get install -y psql"
    When the run is planned
    Then the staged script for "apt-get update\napt-get install -y psql" is labelled "apt-get update"
