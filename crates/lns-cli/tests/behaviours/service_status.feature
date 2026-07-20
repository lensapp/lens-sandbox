Feature: service status
  `lns service status` reports the running service's health — pid,
  uptime, version — and where credential value decisions are stored:
  the OS keychain when one is reachable, otherwise the plaintext
  JSON file.

  Scenario: Status shows credential values protected by the OS keychain
    Given the background service reports the OS keychain credential backend
    When the developer runs "lns service status"
    Then the status output notes credential values are stored in the OS keychain

  Scenario: Status shows credential values resting in the plaintext file
    Given the background service reports the plaintext file credential backend
    When the developer runs "lns service status"
    Then the status output notes credential values are stored in a plaintext file
