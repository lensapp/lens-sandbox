@microvm
Feature: declared developer tools reach a real guest
  Tools declared under `spec.tools` are provisioned on the host and
  injected read-only into the workload guest: they land on the PATH ahead
  of the base image's own copies, and a pulled sandbox's pinned tools are
  pre-provisioned so it starts offline.

  Scenario: A declared tool is available to the workload
    Given the Lens Sandbox service is running
    And a lns.yaml declaring tools ["node@22"] over a base image that ships no node
    When the sandbox runs "node --version"
    Then it prints a node 22 version

  Scenario: Provisioning is disclosed, audited, and reused on the next run
    Given a clean lns cache home
    And the Lens Sandbox service is running in that home
    And a lns.yaml declaring tools ["node@22"] over a base image that ships no node
    When the sandbox runs "node --version"
    Then it prints a node 22 version
    And the run summary discloses the declared tools
    And the audit chain records the tool provisioning
    When the sandbox runs "node --version" again
    Then it prints a node 22 version
    And nothing is provisioned again

  Scenario: A declared tool wins over the base image's copy
    Given the Lens Sandbox service is running
    And a base image that ships node 20 and a lns.yaml declaring tools ["node@22"]
    When the sandbox runs "node --version"
    Then it prints a node 22 version

  Scenario: A pulled sandbox with tools starts offline
    Given the Lens Sandbox service is running
    And a published sandbox declaring pinned tools
    And I ran "lns pull" on its reference while online
    When I run the sandbox with no network available
    Then it starts and the declared tools are available to the workload
