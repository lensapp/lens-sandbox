Feature: the sandbox is the one noun, on a two-tier surface
  `lns` exposes a single user-facing noun — the sandbox. The top level
  carries only docker-familiar verbs, each an exact shortcut into the
  complete `lns sandbox` namespace; lns-native verbs with no docker
  analog (validate, ls, prune) live only under `lns sandbox`.

  Scenario: the front page lists only the docker verbs and the own groups
    When I run "lns --help"
    Then the exit code is 0
    And the output contains "run"
    And the output contains "pull"
    And the output contains "push"
    And the output contains "tag"
    And the output contains "init"
    And the output contains "ps"
    And the output contains "sandbox"
    And the output contains "volume"
    And the output contains "policy"
    And the output contains "integration"

  Scenario: the front page hides the sandbox-only verbs
    When I run "lns --help"
    Then the exit code is 0
    And the output does not contain "validate"
    And the output does not contain "diff"

  Scenario: the sandbox namespace lists the shipped grouped surface
    When I run "lns sandbox --help"
    Then the exit code is 0
    And the output contains "init"
    And the output contains "validate"
    And the output contains "pull"
    And the output contains "push"
    And the output contains "tag"
    And the output contains "ps"
    And the output contains "exec"
    And the output contains "logs"
    And the output contains "attach"
    And the output contains "stop"
    And the output contains "kill"
    And the output contains "ls"
    And the output contains "inspect"
    And the output contains "rm"
    And the output contains "prune"

  Scenario: there is no lns image command
    When I run "lns image ls"
    Then the exit code is 2
    And the output contains "unrecognized subcommand"

  Scenario: the pre-namespace lns ls alias is gone
    When I run "lns ls"
    Then the exit code is 2
    And the output contains "unrecognized subcommand"

  Scenario: sandbox rename is gone
    When I run "lns sandbox rename 3 newname"
    Then the exit code is 2
    And the output contains "unrecognized subcommand"

  @todo
  Scenario Outline: a top-level verb is an exact shortcut into its sandbox form
    Given the service is ready to record the request
    When the user runs "lns <verb> <args>"
    And the user runs "lns sandbox <verb> <args>"
    Then both invocations issue the same request to the service
    Examples:
      | verb    | args        |
      | run     | some-ref    |
      | pull    | some-ref    |
      | ps      |             |
      | stop    | 3           |
      | kill    | 3           |
      | rm      | some-ref    |
      | inspect | some-ref    |

  @todo
  Scenario: run and diff join the sandbox namespace
    When I run "lns sandbox --help"
    Then the exit code is 0
    And the output contains "run"
    And the output contains "diff"

  @todo
  Scenario: sandbox stats is gone and points at ps
    When I run "lns sandbox stats 3"
    Then the exit code is 2
    And the output contains "lns ps"
