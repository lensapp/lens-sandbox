@microvm
Feature: the project policy reaches the guest gate intact
  The host relays the policy to the in-guest proxy over the session channel.
  lens-sandbox-core requires an explicit transport at the default and per-route
  level and fail-closes a non-deny verdict to deny when one is missing, so this
  pins the wire contract from a real boot: an ask-default policy with a
  direct-transport route must arrive intact, not degrade to deny-all.

  Scenario: an ask-default policy with a direct-transport route is accepted by the guest gate
    Given the LNS service is running
    And a network policy holding an ask default and a direct-transport allow route
    When the user runs a microVM command "/bin/sh -c 'echo policy-intact-$((5*5))'"
    Then the exit code is 0
    And the output contains "policy-intact-25"
    And the output does not contain "forcing deny"
