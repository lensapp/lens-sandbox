Feature: two things carry a name, and every command acts on one of them
  An artifact is the `lns.run/v1` document, of whatever kind; a sandbox is
  what a sandbox artifact becomes when it runs. Each has its namespace —
  `lns artifact` and `lns sandbox` — and the top level carries the common
  verbs as exact shortcuts into whichever namespace owns them.

  Scenario: the front page carries the shortcuts and the namespaces
    When I run "lns --help"
    Then the exit code is 0
    And the output contains "run"
    And the output contains "pull"
    And the output contains "push"
    And the output contains "tag"
    And the output contains "init"
    And the output contains "ps"
    And the output contains "sandbox"
    And the output contains "artifact"
    And the output contains "volume"
    And the output contains "connector"

  Scenario: the front page carries no verb that has no shortcut
    When I run "lns --help"
    Then the exit code is 0
    And the output does not contain "diff"

  Scenario: the artifact namespace answers for the document, of any kind
    When I run "lns artifact --help"
    Then the exit code is 0
    And the output contains "init"
    And the output contains "validate"
    And the output contains "pull"
    And the output contains "push"
    And the output contains "tag"
    And the output contains "ls"
    And the output contains "inspect"
    And the output contains "rm"
    And the output contains "prune"

  Scenario: the sandbox namespace answers only for what a document became
    When I run "lns sandbox --help"
    Then the exit code is 0
    And the output contains "run"
    And the output contains "exec"
    And the output contains "logs"
    And the output contains "attach"
    And the output contains "stop"
    And the output contains "kill"
    And the output contains "ls"
    And the output contains "inspect"
    And the output contains "rm"

  Scenario: the document verbs left the sandbox namespace
    When I run "lns sandbox --help"
    Then the exit code is 0
    And the output does not contain "validate"
    And the output does not contain "push"
    And the output does not contain "tag"

  Scenario: the sandbox namespace keeps no ps of its own
    When I run "lns sandbox ps"
    Then the exit code is 2
    And the output contains "unrecognized subcommand"

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

  Scenario Outline: a top-level verb is an exact shortcut into its namespaced form
    Given the service is ready to record the request
    When the user runs the shortcut "lns <verb> <args>"
    And the user runs its namespaced form "lns <namespace> <canonical> <args>"
    Then both invocations issue the same request to the service
    # run's two spellings share the interactive launch path; pinned in sandbox_run.feature.
    # rm and inspect are in both namespaces, so their shortcut arbitrates instead of aliasing.
    Examples:
      | verb    | namespace | canonical | args     |
      | pull    | artifact  | pull      | some-ref |
      | ps      | sandbox   | ls        |          |
      | stop    | sandbox   | stop      | 3        |
      | kill    | sandbox   | kill      | 3        |

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

  Scenario: a path-shaped operand is the document, and the service is never asked
    Given a valid lns.yaml in the current directory
    When the user runs artifact command "inspect ."
    Then the exit code is 0
    And the service received no request

  Scenario: a word only the sandbox namespace knows runs there
    Given the reference "reviewer" resolves to a running sandbox
    When the user runs "lns rm reviewer"
    Then the command fails with an exit code other than 0
    And the output contains "running sandbox"

  Scenario: a word only the artifact namespace knows runs there
    Given the sandbox "hermes:1.4.0" is cached and no other sandbox shares its base-image layers
    When the user runs "lns rm hermes:1.4.0"
    Then the exit code is 0
    And the output contains "removed"

  Scenario: a word both namespaces know is refused, naming the two commands
    Given "hermes" names both a sandbox and a cached artifact
    When the user runs "lns rm hermes"
    Then the command fails with an exit code other than 0
    And the output contains "lns sandbox rm hermes"
    And the output contains "lns artifact rm hermes"

  Scenario: a word neither namespace knows names both as searched
    Given "ghost" names neither a sandbox nor a cached artifact
    When the user runs "lns inspect ghost"
    Then the command fails with an exit code other than 0
    And the output contains "lns sandbox ls"
    And the output contains "lns artifact ls"
