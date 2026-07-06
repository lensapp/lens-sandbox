@todo
Feature: run flags for typed artifacts
  `lns run` gains three artifact-aware levers that stay out of the way for
  a plain image run: --with adds or overrides a mounted component at launch
  and is repeatable; --insecure skips signature verification for the run;
  and --policy, already a file path, also accepts a policy artifact ref.
  These scenarios pin the CLI boundary — parsing and the printed run
  summary; the assembly and verification behaviour is on the service side.

  Scenario: --with and --insecure are documented in run help
    When I run "lns run --help"
    Then the exit code is 0
    And the output contains "--with"
    And the output contains "--insecure"

  Scenario: A --with override is shown in the run summary
    Given the command is `lns run some-registry.example/some-agent:research --with some-registry.example/skills/deep@sha256:abcd`
    When the summary is printed
    Then the summary lists the override "some-registry.example/skills/deep@sha256:abcd"

  Scenario: Repeated --with overrides are all shown in the run summary
    Given the command is `lns run some-registry.example/some-agent:research --with some-registry.example/skills/a:1 --with some-registry.example/skills/b:1`
    When the summary is printed
    Then the summary lists the override "some-registry.example/skills/a:1"
    And the summary lists the override "some-registry.example/skills/b:1"

  Scenario: --insecure is reflected in the run summary
    Given the command is `lns run some-registry.example/some-agent:research --insecure`
    When the summary is printed
    Then the summary states signature verification is skipped

  Scenario: --policy accepts a policy artifact reference and names it as the source
    Given the command is `lns run some-registry.example/some-agent:research --policy some-registry.example/policies/strict:1`
    When the summary is printed
    Then the summary names the policy source "some-registry.example/policies/strict:1"