Feature: the inspect shortcut carries both namespaces' flags

  §1.3: `lns inspect` is an exact alias for whichever namespaced spelling
  the target settles on, so it takes each namespace's flags, and refuses a
  flag the settled target does not take — by name, not by parser accident.

  Scenario: --format on a named document is refused by name, exit 2
    Given a clean lns cache home
    When I run "lns inspect ./lns.yaml --format json"
    Then the exit code is 2
    And the output contains "renders as its author wrote it"

  Scenario: --format with no operand is the default document, refused the same way
    Given a clean lns cache home
    When I run "lns inspect --format json"
    Then the exit code is 2
    And the output contains "renders as its author wrote it"
