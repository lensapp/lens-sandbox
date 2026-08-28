Feature: previewing a composition against a local document
  `inspect` renders a local document offline. The mixins the document declares
  by path, and the ones `--mixin` names, are on this machine, so they merge by
  the same rules a run merges them with and an author sees the composition
  before booting it. A published reference resolves against nothing offline: a
  flag naming one is refused, and one the document declares is listed unmerged.

  Scenario: a mixin named by path merges into the document
    Given a valid lns.yaml in the current directory
    And the mixin "./obs" declares tool "node@22"
    When the user runs artifact command "inspect --mixin ./obs"
    Then the exit code is 0
    And the output contains "tool: node@22"
    And the service received no request

  Scenario: a mixin named by its own document file merges too
    Given a valid lns.yaml in the current directory
    And the mixin "./obs" declares tool "node@22"
    When the user runs artifact command "inspect --mixin ./obs/lns.yaml"
    Then the exit code is 0
    And the output contains "tool: node@22"

  Scenario: the mixin wins where it and the document answer the same question
    Given a lns.yaml declaring tools ["node@20"]
    And the mixin "./obs" declares tool "node@22"
    When the user runs artifact command "inspect --mixin ./obs"
    Then the exit code is 0
    And the output contains "tool: node@22"
    And the output does not contain "node@20"

  Scenario: each flag merges in the order the user gave it
    Given a valid lns.yaml in the current directory
    And the mixin "./first" declares tool "node@20"
    And the mixin "./second" declares tool "node@22"
    When the user runs artifact command "inspect --mixin ./first --mixin ./second"
    Then the exit code is 0
    And the output contains "tool: node@22"
    And the output does not contain "node@20"

  Scenario: a mixin the mixin names is merged as well
    Given a valid lns.yaml in the current directory
    And the mixin "./obs" declares tool "node@22"
    And the mixin "./obs" layers on "../deep"
    And the mixin "./deep" declares tool "python@3.12"
    When the user runs artifact command "inspect --mixin ./obs"
    Then the exit code is 0
    And the output contains "tool: node@22"
    And the output contains "tool: python@3.12"

  Scenario: a published mixin reference is refused, and the message says why
    Given a valid lns.yaml in the current directory
    When the user runs artifact command "inspect --mixin ghcr.io/acme/obs:2"
    Then the command fails with an exit code other than 0
    And the output contains "an offline render resolves nothing"
    And the output contains "ghcr.io/acme/obs:2"
    And the service received no request

  Scenario: a mixin path that holds no document names the path
    Given a valid lns.yaml in the current directory
    When the user runs artifact command "inspect --mixin ./absent"
    Then the command fails with an exit code other than 0
    And the output contains "/work/absent"

  Scenario: a mixin path holding a sandbox is refused
    Given a valid lns.yaml in the current directory
    And the mixin "./obs" holds a sandbox document
    When the user runs artifact command "inspect --mixin ./obs"
    Then the command fails with an exit code other than 0
    And the output contains "is not a mixin"

  Scenario: malformed yaml in the mixin is reported against the mixin
    Given a valid lns.yaml in the current directory
    And the mixin "./obs" holds malformed yaml
    When the user runs artifact command "inspect --mixin ./obs"
    Then the command fails with an exit code other than 0
    And the output contains "parsing"
    And the output contains "/work/obs/lns.yaml"

  Scenario: the flag reaches a document -f names too
    Given a sandbox definition file "lns.dev.yaml" in the current directory
    And the mixin "./obs" declares tool "node@22"
    When the user runs artifact command "inspect -f lns.dev.yaml --mixin ./obs"
    Then the exit code is 0
    And the output contains "tool: node@22"

  Scenario: a mixin the document declares by path merges too
    Given an lns.yaml layering on "./obs"
    And the mixin "./obs" declares tool "node@22"
    When the user runs artifact command "inspect"
    Then the exit code is 0
    And the output contains "mixin: /work/obs/lns.yaml"
    And the output contains "tool: node@22"
    And the service received no request

  Scenario: a flag merges after the mixins the document declares
    Given an lns.yaml layering on "./first"
    And the mixin "./first" declares tool "node@20"
    And the mixin "./first" also declares tool "python@3.12"
    And the mixin "./second" declares tool "node@22"
    When the user runs artifact command "inspect --mixin ./second"
    Then the exit code is 0
    And the output contains "tool: python@3.12"
    And the output contains "tool: node@22"
    And the output does not contain "node@20"

  Scenario: a mixin the document declares by published reference is listed, not merged
    Given an lns.yaml layering on "ghcr.io/acme/obs@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    When the user runs artifact command "inspect"
    Then the exit code is 0
    And the output contains "mixin: ghcr.io/acme/obs@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa (published; not merged, because this render is offline)"
    And the service received no request

  Scenario: a mixin a declared mixin names by published reference is listed too
    Given an lns.yaml layering on "./obs"
    And the mixin "./obs" declares tool "node@22"
    And the mixin "./obs" layers on "ghcr.io/acme/deep@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    When the user runs artifact command "inspect"
    Then the exit code is 0
    And the output contains "tool: node@22"
    And the output contains "mixin: ghcr.io/acme/deep@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa (published; not merged"

  Scenario: a declared mixin path that holds no document names the path
    Given an lns.yaml layering on "./absent"
    When the user runs artifact command "inspect"
    Then the command fails with an exit code other than 0
    And the output contains "/work/absent"

  Scenario: a declared mixin holding a sandbox is refused
    Given an lns.yaml layering on "./obs"
    And the mixin "./obs" holds a sandbox document
    When the user runs artifact command "inspect"
    Then the command fails with an exit code other than 0
    And the output contains "is not a mixin"

  Scenario: malformed yaml in a declared mixin is reported against it
    Given an lns.yaml layering on "./obs"
    And the mixin "./obs" holds malformed yaml
    When the user runs artifact command "inspect"
    Then the command fails with an exit code other than 0
    And the output contains "parsing"
    And the output contains "/work/obs/lns.yaml"

  Scenario: a declared mixin that names itself is refused
    Given an lns.yaml layering on "./obs"
    And the mixin "./obs" declares tool "node@22"
    And the mixin "./obs" layers on "./"
    When the user runs artifact command "inspect"
    Then the command fails with an exit code other than 0
    And the output contains "reachable from itself"

  Scenario: a declared mixin roots at the document that declares it
    Given the definition file "sub/lns.dev.yaml" layers on "./obs"
    And the mixin "./sub/obs" declares tool "node@22"
    When the user runs artifact command "inspect -f sub/lns.dev.yaml"
    Then the exit code is 0
    And the output contains "tool: node@22"

  Scenario: a flag roots where the user typed it, not at the document -f names
    Given the definition file "sub/lns.dev.yaml" layers on nothing
    And the mixin "./obs" declares tool "node@22"
    When the user runs artifact command "inspect -f sub/lns.dev.yaml --mixin ./obs"
    Then the exit code is 0
    And the output contains "tool: node@22"
