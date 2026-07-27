Feature: declared developer tools are provisioned once per machine, outside workload policy
  A sandbox definition lists under `spec.tools` the developer tools its
  workload needs. The service provisions them before the microVM boots:
  acquisition is audited system egress — the same trust shape as pulling
  `spec.image`, declaration is consent — so it raises no approval card and
  needs no policy route. Provisioned tool sets are cached per machine and
  reused without network; the workload receives them read-only, and its
  own traffic stays inside the normal policy cage.

  @todo
  Scenario: Tool acquisition needs no policy route and raises no approval
    Given a lns.yaml declaring tools ["node@22"] with defaultVerdict ask and no allowedRoutes
    When I run the sandbox for the first time
    Then the tools are provisioned without any approval card
    And the workload's own network requests still ask as usual

  @todo
  Scenario: A provisioned tool set is reused without network
    Given tools ["node@22"] were provisioned by an earlier run on this machine
    When I run the sandbox again
    Then the run starts without downloading anything

  @todo
  Scenario: Failed provisioning refuses the launch cleanly
    Given a lns.yaml declaring a tool whose download cannot complete
    When I run the sandbox
    Then the launch is refused naming the tool and the cause
    And a later run retries from a clean state

  Scenario: A tool unknown to the registry refuses the launch
    Given a lns.yaml declaring tools ["definitely-not-a-tool@1"]
    When I run the sandbox
    Then the launch is refused naming the unknown tool

  @todo
  Scenario: First provision records resolved versions on this machine
    Given a lns.yaml declaring tools ["node@22"]
    When I run the sandbox for the first time
    Then the resolved exact version is recorded on this machine
    And later runs here use the recorded version even after upstream releases a newer 22.x

  @todo
  Scenario: Tool provisioning is audited
    When tools are provisioned for a run
    Then the audit chain records what was fetched, from where, and its resolved version
