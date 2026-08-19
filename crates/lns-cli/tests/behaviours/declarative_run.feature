Feature: applying declarative sandbox launch settings
  Workdir and mounts in lns.yaml are portable launch settings. Relative bind
  sources are rooted in the consumer's project directory, explicit run flags
  win per setting and per guest target, and declarative binds enter the same
  host-side secret decision flow as flag-provided binds.

  Scenario: a local definition supplies workdir, a project bind, and a named volume
    Given an lns.yaml declaring workdir and declarative mounts
    When the local sandbox launch settings are resolved with no overrides
    Then the resolved workdir is "/workspace"
    And the resolved host binds are exactly "/work -> /workspace"
    And the resolved volumes are exactly "some-cache:/home/node/.cache:ro"

  Scenario: a declared volume size reaches the launch request
    Given an lns.yaml declaring a volume sized 40Gi
    When the local sandbox launch settings are resolved with no overrides
    Then the resolved volumes are exactly "some-cache:/home/node/.cache:40Gi"

  Scenario: a pulled sandbox's declared volume size reaches the consumer's launch request
    Given an lns.yaml declaring a volume sized 40Gi
    When the published sandbox launch settings are resolved from "/consumer/project"
    Then the resolved volumes are exactly "some-cache:/home/node/.cache:40Gi"

  Scenario: a volume that declares no size asks the service for nothing in particular
    Given an lns.yaml declaring workdir and declarative mounts
    When the local sandbox launch settings are resolved with no overrides
    Then the resolved volumes are exactly "some-cache:/home/node/.cache:ro"

  Scenario: explicit workdir and mount flags override matching declarative settings
    Given an lns.yaml declaring workdir and declarative mounts
    When the local sandbox launch settings are resolved with "--workdir /src -v cli-cache:/home/node/.cache -v /other:/workspace:ro"
    Then the resolved workdir is "/src"
    And the resolved host binds are exactly "/other -> /workspace:ro"
    And the resolved volumes are exactly "cli-cache:/home/node/.cache"

  Scenario: an unrelated CLI mount is kept alongside declarative mounts
    Given an lns.yaml declaring workdir and declarative mounts
    When the local sandbox launch settings are resolved with "-v scratch:/scratch"
    Then the resolved host binds are exactly "/work -> /workspace"
    And the resolved volumes are exactly "some-cache:/home/node/.cache:ro, scratch:/scratch"

  Scenario: a published definition roots its relative bind in the consumer project
    Given a published sandbox declaring a relative bind and workdir
    When the published sandbox launch settings are resolved from "/consumer/project"
    Then the resolved workdir is "/workspace"
    And the resolved host binds are exactly "/consumer/project -> /workspace"

  Scenario: a declared exclude drops the subpath with no prompt
    Given an lns.yaml declaring a bind excluding ".cargo"
    And the host directory "/work" contains ".cargo"
    When the declarative host binds are resolved interactively
    Then ".cargo" is dropped from the bind
    And no KEEP or DROP prompt is shown

  Scenario: a published sandbox's declared exclude drops the subpath too
    Given an lns.yaml declaring a bind excluding ".cargo"
    And the host directory "/work" contains ".cargo"
    When the published host binds are resolved interactively
    Then ".cargo" is dropped from the bind
    And no KEEP or DROP prompt is shown

  Scenario: bind source text is not shell or environment interpolated
    Given an lns.yaml declaring a bind source "$PWD"
    When the local sandbox launch settings are resolved with no overrides
    Then the resolved host binds are exactly "/work/$PWD -> /workspace"

  Scenario: a declarative bind uses the host secret decision flow
    Given an lns.yaml declaring workdir and declarative mounts
    And the host directory "/work" contains ".env"
    And the operator will answer the secret prompt with "drop"
    When the declarative host binds are resolved interactively
    Then ".env" is dropped from the bind
    And a per-machine DROP decision is recorded for "/work/.env"
