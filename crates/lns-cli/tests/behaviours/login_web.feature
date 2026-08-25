Feature: browser login when `lns login` is run without credential flags
  A plain `lns login` — no username, no password, no piped secret — starts a
  browser device flow against the registry: it prints a one-time
  confirmation code, opens the browser, and waits until the registry issues a
  fresh token for the signed-in account. The issued credential then rides the
  existing path through the service, which verifies it against the registry
  and stores it only when it is accepted. Any credential flag keeps the
  traditional flag-driven path unchanged.

  Scenario: a flagless login runs the web flow and the service stores the issued credential
    Given the web flow will issue a token for "webuser"
    When I log in with "lns login hub.lns.run"
    Then the exit code is 0
    And the output contains "Your one-time confirmation code is:"
    And the output contains "You are now logged in to hub.lns.run as webuser."
    And the service saw the web-issued credential for "webuser"

  Scenario: credential flags keep the traditional flag-driven path
    Given the web flow would panic if it were consulted
    When I log in with "lns login -u me -p tok hub.lns.run"
    Then the exit code is 0
    And the output contains "You are now logged in to hub.lns.run as me."
    And the output does not contain "confirmation code"

  Scenario: a username alone still demands a password source
    Given the web flow would panic if it were consulted
    When I log in with "lns login -u me hub.lns.run"
    Then the exit code is 1
    And the output contains "a password is required"

  Scenario: a registry without browser login explains the flag-driven fallback
    Given the web flow reports the registry does not support it
    When I log in with "lns login registry.example.test"
    Then the exit code is 1
    And the output contains "registry.example.test does not support browser login"
    And the output contains "--password-stdin"

  Scenario: a login denied in the browser stores nothing
    Given the web flow reports the browser denied the login
    When I log in with "lns login hub.lns.run"
    Then the exit code is 1
    And the output contains "denied in the browser"
    And the service stored no credential

  Scenario: an expired confirmation code asks the user to retry
    Given the web flow reports the confirmation code expired
    When I log in with "lns login hub.lns.run"
    Then the exit code is 1
    And the output contains "run `lns login` again"
