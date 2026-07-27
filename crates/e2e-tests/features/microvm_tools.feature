@microvm
Feature: declared developer tools reach a real guest
  Tools declared under `spec.tools` are provisioned on the host and
  injected read-only into the workload guest: they land on the PATH ahead
  of the base image's own copies, and a pulled sandbox's pinned tools are
  pre-provisioned so it starts offline.

  Scenario: A declared tool is available to the workload
    Given a lns.yaml declaring tools ["node@22"] over a base image that ships no node
    When the sandbox runs "node --version"
    Then it prints a node 22 version

  Scenario: A declared tool wins over the base image's copy
    Given a base image that ships node 20 and a lns.yaml declaring tools ["node@22"]
    When the sandbox runs "node --version"
    Then it prints a node 22 version

  Scenario: A pulled sandbox with tools starts offline
    Given a published sandbox declaring pinned tools
    And I ran "lns pull" on its reference while online
    When I run the sandbox with no network available
    Then it starts and the declared tools are available to the workload
