Feature: declared developer tools are provisioned once per machine, outside workload policy
  A sandbox definition lists under `spec.tools` the developer tools its
  workload needs. The service provisions them before the microVM boots, in a
  disposable guest of its own: declaration is consent, so it raises no approval
  card and needs no policy route — not even the strictest policy gates it. That
  no card is raised is structural rather than checked here: provisioning runs in
  a separate guest with no supervisor and no policy gate, so there is no path
  from this pre-boot work to an approval session for a scenario to observe. What
  this feature pins is that the policy does not *refuse* the install.
  Provisioned tool sets are cached per machine and reused without network; the
  workload receives them read-only, and its own traffic stays inside the normal
  policy cage (covered against a live guest by the @microvm suite).

  Scenario: The strictest policy does not gate provisioning
    Given a lns.yaml declaring tools ["node@22"] with defaultVerdict deny and no allowedRoutes
    When I run the sandbox for the first time
    Then the tools are provisioned under it anyway

  Scenario: A provisioned tool set is reused without network
    Given tools ["node@22"] were provisioned by an earlier run on this machine
    When I run the sandbox again
    Then the run starts without downloading anything

  Scenario: Failed provisioning refuses the launch cleanly
    Given a lns.yaml declaring a tool whose download cannot complete
    When I run the sandbox
    Then the launch is refused naming the tool and the cause
    And a later run retries from a clean state

  Scenario: A tool unknown to the registry refuses the launch
    Given a lns.yaml declaring tools ["definitely-not-a-tool@1"]
    When I run the sandbox
    Then the launch is refused naming the unknown tool

  Scenario: First provision records resolved versions on this machine
    Given a lns.yaml declaring tools ["node@22"]
    When I run the sandbox for the first time
    Then the resolved exact version is recorded on this machine
    And later runs here use the recorded version even after upstream releases a newer 22.x

  Scenario: Tool provisioning is audited
    When tools are provisioned for a run
    Then the audit chain records what was fetched, from where, and its resolved version
