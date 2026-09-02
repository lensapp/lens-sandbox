Feature: distributing a sandbox through a registry end to end
  push uploads ./lns.yaml as a sandbox artifact; pull fetches it back into
  the daemon's cache over the real Unix-socket IPC and real (loopback)
  registry HTTP; tag, ls, inspect, rm, and prune then manage the cached
  copy. All of it runs virt-free, and each verb here uses its top-level
  shortcut form to prove the shortcut tier is wired end to end.

  Background:
    Given a clean lns cache home
    And a local registry
    And the LNS service is running in that home
    And the user pushes a sandbox built from ./lns.yaml in one step

  Scenario: pull fetches the pushed sandbox; its base image stays cache-internal
    When I run lns "pull <pushed-ref>" against the service
    Then the exit code is 0
    And the output contains "pulled"
    And the output contains the pushed reference
    When I run lns "artifact ls" against the service
    Then the exit code is 0
    And the output contains the pushed reference
    And the output contains "sandbox"
    And the output does not contain "/e2e-base@sha256:"
    And the output does not contain "image"

  Scenario: tag re-references the cached sandbox under a new tag
    When I run lns "pull <pushed-ref>" against the service
    And I run lns "tag <pushed-ref> <pushed-ref>-copy" against the service
    Then the exit code is 0
    When I run lns "artifact ls" against the service
    Then the output contains "e2e-cache-sandbox:1-copy"

  Scenario: inspect renders the cached sandbox's definition
    When I run lns "pull <pushed-ref>" against the service
    And I run lns "inspect <pushed-ref>" against the service
    Then the exit code is 0
    And the output contains "kind: sandbox"
    And the output contains the pushed reference
    And the output contains "workdir: /workspace"
    And the output contains "bind . -> /workspace"
    And the output contains "volume e2e-cache -> /home/sandbox/.cache"

  Scenario: a path fileset publishes inside the sandbox artifact and discloses on inspect
    When the user pushes a sandbox declaring a path fileset in one step
    And I run lns "pull <pushed-ref>" against the service
    Then the exit code is 0
    When I run lns "inspect <pushed-ref>" against the service
    Then the exit code is 0
    And the output contains "fileset: ./skills"
    And the output contains "/opt/agent-skills"

  Scenario: an inline fileset round-trips inside the sandbox artifact
    When the user pushes a sandbox declaring a root-owned inline file with content "do-not-print" in one step
    Then nothing but the sandbox artifact is uploaded
    When I run lns "pull <pushed-ref>" against the service
    Then the exit code is 0
    When I run lns "inspect <pushed-ref>" against the service
    Then the exit code is 0
    And the output contains "fileset: inline"
    And the output contains "/etc/agent"
    And the output contains "owner: root"
    And the output does not contain "do-not-print"

  Scenario: rm removes the cached sandbox
    When I run lns "pull <pushed-ref>" against the service
    And I run lns "artifact rm <pushed-ref>" against the service
    Then the exit code is 0
    When I run lns "artifact ls" against the service
    Then the output no longer lists the pushed reference

  Scenario: prune reclaims the cached sandbox
    When I run lns "pull <pushed-ref>" against the service
    And I run lns "artifact prune --force" against the service
    Then the exit code is 0
    And the output contains "reclaimed"
    When I run lns "artifact ls" against the service
    Then the output no longer lists the pushed reference
