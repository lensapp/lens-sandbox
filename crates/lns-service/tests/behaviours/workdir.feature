Feature: lns run working directory — image WORKDIR honored, -w overrides
  The workload starts in the image's WORKDIR by default, mirroring
  `docker run`. `lns run -w DIR` overrides it. For supervised (policy)
  runs the agent's cwd is pinned through the supervisor's environment,
  so a user `-e WORKSPACE_PATH=…` can never redirect where the agent
  actually starts.

  Scenario: The image WORKDIR is the default working directory
    Given the image declares WORKDIR /srv
    When the working directory is resolved for `lns run someimage`
    Then the workload working directory is /srv

  Scenario: -w overrides the image WORKDIR
    Given the image declares WORKDIR /srv
    When the working directory is resolved for `lns run -w /app someimage`
    Then the workload working directory is /app

  Scenario: No -w and no image WORKDIR leaves the working directory unset
    When the working directory is resolved for `lns run someimage`
    Then no working directory is forced on the workload

  Scenario: A supervised agent's cwd is pinned through its environment
    When the user runs `lns run -w /app someimage` under a policy
    Then the supervised workload env pins WORKSPACE_PATH to "/app"

  Scenario: -e WORKSPACE_PATH cannot redirect a -w working directory
    When the user runs `lns run -w /app -e WORKSPACE_PATH=/evil someimage` under a policy
    Then the supervised workload env pins WORKSPACE_PATH to "/app"

  Scenario: An unsupervised run carries no supervisor cwd variable
    When the user runs `lns run -w /app someimage` without a policy
    Then the workload env carries no WORKSPACE_PATH entry
