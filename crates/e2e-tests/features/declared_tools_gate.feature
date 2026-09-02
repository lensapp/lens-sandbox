Feature: declared tools gate the launch before boot
  The unknown-tool and unsupported-backend refusals fire on the host at
  launch planning, before any microVM boots or any byte is fetched, so these
  wiring confirmations run virt-free through the real lns and lns-service
  binaries. Publish-time pinning likewise runs against the in-process
  registry and a loopback stand-in for the tool version index.

  Scenario: an unknown declared tool refuses the launch
    Given a clean lns cache home
    And the LNS service is running in that home
    And a lns.yaml declaring tools ["definitely-not-a-tool@1"]
    When the user runs the sandbox definition
    Then the exit code is non-zero
    And the output contains "definitely-not-a-tool"
    And the output contains "not a tool lns can provision"

  Scenario: a plugin-backed tool is refused with the image remedy
    Given a clean lns cache home
    And the LNS service is running in that home
    And a lns.yaml declaring a registry tool with an unsupported backend
    When the user runs the sandbox definition
    Then the exit code is non-zero
    And the output names that declared tool
    And the output contains "bring it via spec.image"

  Scenario: push pins the resolved tool version into the published config
    Given a local registry
    And a version index resolving "node" to versions "22.9.0,22.11.0,23.0.0"
    When the user pushes a sandbox declaring tool "node@22" in one step
    Then the pushed config pins tool "node@22.11.0"
