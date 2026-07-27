Feature: declaring developer tools in a sandbox definition
  A sandbox definition lists under `spec.tools` the developer tools its
  workload needs, as portable `name@version` entries (`node@22`,
  `python@3.12`, `node@latest`). The shape is checked offline — a version
  is always required — while resolution happens at run time.

  Scenario: Declaring tools validates offline
    Given a lns.yaml declaring tools ["node@22", "python@3.12"]
    When the user runs sandbox command "validate"
    Then validation succeeds without touching the network or the service

  Scenario: A malformed tool entry is refused with its cause
    Given a lns.yaml declaring tools ["node@"]
    When the user runs sandbox command "validate"
    Then validation fails naming the entry and the expected "name@version" shape

  Scenario: A tool entry without a version is refused
    Given a lns.yaml declaring tools ["node"]
    When the user runs sandbox command "validate"
    Then validation fails asking for an explicit version such as "node@22" or "node@latest"
