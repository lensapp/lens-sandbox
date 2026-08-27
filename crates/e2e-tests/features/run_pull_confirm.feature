Feature: running a pulled sandbox asks before mounting into the workload
  A published sandbox can declare host binds, named volumes, and filesets.
  Running it by reference discloses what it will mount on this machine and
  requires consent; with no terminal the run fails closed — before anything
  is mounted, attached, or booted — and names --yes as the escape hatch.
  Detaching is no answer to it: -d decides who holds the terminal after
  boot, and every question is asked before that.
  All of it is virt-free: the refusal happens ahead of any VM work.

  Background:
    Given a clean lns cache home
    And a local registry
    And the Lens Sandbox service is running in that home

  Scenario: a pulled sandbox declaring a bind and a volume fails closed without a terminal
    Given the user pushes a sandbox built from ./lns.yaml in one step
    When I run "run <pushed-ref>" in the project directory
    Then the exit code is non-zero
    And the output contains "declares these effects:"
    And the output contains "the workload can read and write this host directory"
    And the output contains "this machine's persistent volume"
    And the output contains "no terminal to confirm"
    And the output contains "--yes"
    And the output does not contain "started run"

  Scenario: a detached run of the same sandbox fails closed for the same reason
    Given the user pushes a sandbox built from ./lns.yaml in one step
    When I run "run -d <pushed-ref>" in the project directory
    Then the exit code is non-zero
    And the output contains "declares these effects:"
    And the output contains "no terminal to confirm"
    And the output contains "--yes"
    And the output does not contain "started run"

  Scenario: a detached run passed --yes is asked nothing
    Given the user pushes a sandbox built from ./lns.yaml in one step
    When I run "run -d --yes <pushed-ref>" in the project directory
    Then the output does not contain "declares these effects:"
    And the output does not contain "no terminal to confirm"
    And the output does not contain "Continue?"

  Scenario: a pulled sandbox declaring a fileset also requires consent
    When the user pushes a sandbox declaring a path fileset in one step
    And I run "run <pushed-ref>" in the project directory
    Then the exit code is non-zero
    And the output contains "declares these effects:"
    And the output contains "author-published files the workload can read and write"
    And the output contains "no terminal to confirm"
    And the output does not contain "started run"
