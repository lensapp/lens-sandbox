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

  The set/clear scenarios use an arbitrary declared `some-provider`; the
  behaviour is identical for any known provider, so nothing here pins a
  specific shipped service.

  Background:
    Given the developer has declared the "some-provider" credential provider

  Scenario: Setting a credential to use the host value persists a host-detect rule
    When the developer sets the "some-provider" credential to use the host value
    Then "~/.lns-credentials.json" gains an entry for "some-provider" with kind "host-detect"

  Scenario: Setting a stored value persists the typed value
    When the developer sets the "some-provider" credential to a stored value "some-secret"
    Then "~/.lns-credentials.json" gains an entry for "some-provider" with kind "stored" carrying "some-secret"

  Scenario: Setting a stored value from stdin keeps the secret off the command line
    When the developer sets the "some-provider" credential to a stored value piped on stdin as "some-piped-secret"
    Then "~/.lns-credentials.json" gains an entry for "some-provider" with kind "stored" carrying "some-piped-secret"

  Scenario: Setting a credential for a catalog integration is accepted
    A connected integration (e.g. the bundled "gitlab") is a known
    credential provider too, so its value can be pre-set non-interactively
    just like any provider — the value arms its injection on next use.
    When the developer sets the "gitlab" credential to a stored value "glpat-real"
    Then "~/.lns-credentials.json" gains an entry for "gitlab" with kind "stored" carrying "glpat-real"

  Scenario: Setting a value from stdin with nothing piped fails clearly
    When the developer sets the "some-provider" credential from empty stdin
    Then the command fails with a clear error mentioning stdin
    And "~/.lns-credentials.json" is unchanged

  Scenario: Passing both --value and --value-stdin is rejected
    When the developer tries to set "some-provider" passing both --value and --value-stdin
    Then the command is rejected for passing two value sources
    And "~/.lns-credentials.json" is unchanged

  Scenario: Denying a credential persists a deny rule
    When the developer denies the "some-provider" credential
    Then "~/.lns-credentials.json" gains an entry for "some-provider" with kind "deny"

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
    Given a sandbox is running with the seeded "some-provider" placeholder and no credential rule for "some-provider"
    When the developer sets the "some-provider" credential to a stored value "some-secret"
    Then "~/.lns-credentials.json" gains an entry for "some-provider" with kind "stored" carrying "some-secret"

  Scenario: Listing credential rules shows kinds without leaking stored values
    Given "~/.lns-credentials.json" has entries: "anthropic" host-detect, "openai" stored, "linear" deny
    When the developer lists credential rules
    Then the output shows the three ids and their kinds
    And the stored value for "openai" is not printed in plain text by default

  # Clearing the decision re-arms the prompt on next use; the running sandbox
  # observes the deletion via its CredentialWatcher (covered in lns-service
  # credential_flow.feature). Here we pin only that the entry is gone.
  Scenario: Clearing a credential rule deletes it from the file
    Given "~/.lns-credentials.json" has a stored rule for "some-provider"
    When the developer clears the "some-provider" credential rule
    Then "~/.lns-credentials.json" no longer contains an entry for "some-provider"
