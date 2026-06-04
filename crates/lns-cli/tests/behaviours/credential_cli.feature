Feature: shaping credential rules from the CLI
  Credential decisions — use the host value, store a typed value, or
  deny — are made interactively in the approval window today. The CLI
  is the non-interactive counterpart: it writes the same decisions into
  `~/.lns-credentials.json`, the per-machine credential store, in the
  exact shape the approval window persists. Real credential material
  stays in that per-machine file and never lands in the shareable
  `lns-policy.yaml`. The CLI writes the file directly; a running
  sandbox's file watcher hot-swaps the change, so a decision made from
  the CLI takes effect without restarting the workload, and removing a
  rule re-arms the prompt on next use exactly as a manual edit does.

  Scenario: Setting a credential to use the host value persists a host-detect rule
    When the developer sets the "github" credential to use the host value
    Then "~/.lns-credentials.json" gains an entry for "github" with kind "host-detect"

  Scenario: Setting a stored value persists the typed value
    When the developer sets the "github" credential to a stored value "ghp_real"
    Then "~/.lns-credentials.json" gains an entry for "github" with kind "stored" carrying "ghp_real"

  Scenario: Setting a stored value from stdin keeps the secret off the command line
    When the developer sets the "github" credential to a stored value piped on stdin as "ghp_piped"
    Then "~/.lns-credentials.json" gains an entry for "github" with kind "stored" carrying "ghp_piped"

  Scenario: Setting a value from stdin with nothing piped fails clearly
    When the developer sets the "github" credential from empty stdin
    Then the command fails with a clear error mentioning stdin
    And "~/.lns-credentials.json" is unchanged

  Scenario: Passing both --value and --value-stdin is rejected
    When the developer tries to set "github" passing both --value and --value-stdin
    Then the command is rejected for passing two value sources
    And "~/.lns-credentials.json" is unchanged

  Scenario: Denying a credential persists a deny rule
    When the developer denies the "github" credential
    Then "~/.lns-credentials.json" gains an entry for "github" with kind "deny"

  Scenario: Setting a rule for an unknown provider is rejected
    Given no provider with id "made-up" is registered as built-in or custom
    When the developer tries to set a credential for "made-up"
    Then the command fails with a clear error naming the unknown id
    And "~/.lns-credentials.json" is unchanged

  # The CLI writes the decision into the file a running sandbox watches; the
  # sandbox then hot-swaps it via its CredentialWatcher. That reload is covered
  # by lns-service credential_flow.feature ("A manual edit to the credentials
  # file is picked up live"), so here we pin only the CLI half.
  Scenario: A CLI credential decision is written to the watched credentials file
    Given a sandbox is running with the seeded "github" placeholder and no credential rule for "github"
    When the developer sets the "github" credential to a stored value "ghp_real"
    Then "~/.lns-credentials.json" gains an entry for "github" with kind "stored" carrying "ghp_real"

  Scenario: Listing credential rules shows kinds without leaking stored values
    Given "~/.lns-credentials.json" has entries: "github" host-detect, "openai" stored, "linear" deny
    When the developer lists credential rules
    Then the output shows the three ids and their kinds
    And the stored value for "openai" is not printed in plain text by default

  # Clearing the decision re-arms the prompt on next use; the running sandbox
  # observes the deletion via its CredentialWatcher (covered in lns-service
  # credential_flow.feature). Here we pin only that the entry is gone.
  Scenario: Clearing a credential rule deletes it from the file
    Given "~/.lns-credentials.json" has a stored rule for "github"
    When the developer clears the "github" credential rule
    Then "~/.lns-credentials.json" no longer contains an entry for "github"
