Feature: lns-service credential flow
  Lens Sandbox controls credential-backed actions the same way it
  controls network destinations: ask once, decide once, remember. At
  run start lns-service seeds the workload's environment with
  value-shaped placeholders for every entry in a built-in, declarative
  registry of known providers (GitHub, OpenAI, Anthropic, …). Real
  credential material never enters the workload — the placeholder is
  what tools see. When the in-sandbox MITM detects a placeholder value
  inside an outbound request and no rule yet authorises it, it holds
  the request and lns-service surfaces a credential card in the
  always-on-top approval window. The card asks the developer for the
  real value: either accept the one detected on the host by the
  provider's per-service detection strategy, type one in, or deny.
  Every decision is sticky — there is no once/always distinction.
  Decisions and any typed values land in `~/.lns-credentials.json` (a
  pluggable host-side store, JSON-file v1). They do NOT live in
  `lns-policy.yaml`: the shareable policy file stays free of
  per-machine credential state. Manual edits to
  `~/.lns-credentials.json` are picked up live and double as the
  revocation mechanism.

  Scenario: Default placeholders are present in the workload from boot
    Given the built-in registry includes "github", "openai", and "anthropic"
    When a workload is launched in the sandbox
    Then the workload's environment contains a value-shaped placeholder for each registry entry
    And no real credential material is present inside the workload
    And no approval card is shown at boot

  Scenario: Each placeholder matches its provider's real value shape
    Given the built-in registry includes "github" and "openai"
    When a workload reads the seeded GITHUB_TOKEN and OPENAI_API_KEY
    Then GITHUB_TOKEN starts with the "ghp_" prefix and is the length of a real GitHub personal access token
    And OPENAI_API_KEY starts with the "sk-" prefix and is the length of a real OpenAI API key

  Scenario: A placeholder used in an outbound request prompts the developer when the host has the credential
    Given a workload is running with the seeded "github" placeholder
    And no credential rule exists in "~/.lns-credentials.json" for "github"
    And the host has a "github" credential reachable via the registered detection strategy
    When the workload sends a request carrying the GitHub placeholder
    Then a credential card appears for "github" showing the provider, the originating sandbox, and the destination
    And the card offers "use from host", a custom-value input, and "deny"
    And the workload's request is held pending a decision

  Scenario: A placeholder used in an outbound request prompts the developer when the host has no credential
    Given a workload is running with the seeded "github" placeholder
    And no credential rule exists in "~/.lns-credentials.json" for "github"
    And no "github" credential is reachable on the host
    When the workload sends a request carrying the GitHub placeholder
    Then a credential card appears for "github"
    And the card offers a custom-value input and "deny"
    And the card states that no host credential was detected
    And the workload's request is held pending a decision

  Scenario: Use from host arms the credential at the boundary and persists a host-detect rule
    Given a credential card for "github" is visible with "use from host" available
    When the developer picks "use from host"
    Then "~/.lns-credentials.json" gains an entry for "github" with kind "host-detect"
    And the workload's request leaves the boundary with the host-detected GitHub credential substituted in
    And the workload still sees only the placeholder
    And a future request carrying the GitHub placeholder is exchanged silently using the currently host-detected value
    And "lns-policy.yaml" is unchanged

  Scenario: A typed value arms the credential at the boundary and persists a stored rule
    Given a credential card for "github" is visible
    When the developer types a value and submits
    Then "~/.lns-credentials.json" gains an entry for "github" with kind "stored" carrying the typed value
    And the workload's request leaves the boundary with the typed value substituted for the placeholder
    And a future request carrying the GitHub placeholder is exchanged silently using the stored value
    And "lns-policy.yaml" is unchanged

  Scenario: Deny fails the held request at the boundary and persists a deny rule
    Given a credential card for "github" is visible
    When the developer picks "deny"
    Then "~/.lns-credentials.json" gains an entry for "github" with kind "deny"
    And the workload's held request is failed at the boundary
    And a future request carrying the GitHub placeholder is failed at the boundary without prompting
    And "lns-policy.yaml" is unchanged

  Scenario: A request needing both an unknown network rule and an unknown credential surfaces both cards in parallel
    Given a workload is running with the seeded "github" placeholder
    And the policy has no rule for "api.github.com"
    And no credential rule exists in "~/.lns-credentials.json" for "github"
    When the workload sends a request to "api.github.com" carrying the GitHub placeholder
    Then a network card appears for "api.github.com"
    And a credential card appears for "github"
    And the workload's request is held until both cards have decisions
    And a "deny" decision on either card fails the request at the boundary

  Scenario: A stored rule whose source no longer yields a value re-prompts
    Given "~/.lns-credentials.json" has an entry for "github" with kind "host-detect"
    And the host no longer yields a "github" credential via the registered detection strategy
    When the workload sends a request carrying the GitHub placeholder
    Then a credential card for "github" appears
    And the card states why a fresh value is required
    And the workload's request is held pending a decision

  Scenario: A manual edit to the credentials file is picked up live and revokes the rule
    Given a workload is running with a "stored" credential rule for "github" in "~/.lns-credentials.json"
    When the developer deletes the "github" entry from "~/.lns-credentials.json"
    Then a subsequent request from the workload carrying the GitHub placeholder fires a fresh credential card for "github"
    And no restart of the workload is required

  Scenario: Repeated placeholder uses share one credential card
    Given a workload sends a request carrying the GitHub placeholder with no credential rule for "github"
    And a credential card for "github" is visible
    When the workload sends a second request carrying the GitHub placeholder before the developer decides
    Then no second credential card appears
    And when the developer's decision is recorded, both requests resolve under that decision

  Scenario: A credential card with no decision after the timeout fails the request closed without persisting
    Given a credential card for "github" is visible
    When no decision is recorded before the configured approval timeout
    Then the workload's held request is failed at the boundary
    And the credential card is removed from the approval window
    And "~/.lns-credentials.json" is unchanged
    And a future request carrying the GitHub placeholder fires a fresh credential card

  Scenario: A workload exit withdraws its open credential cards without persisting
    Given a workload has an open credential card for "github"
    When the workload exits before a decision is recorded
    Then the credential card is removed from the approval window
    And "~/.lns-credentials.json" is unchanged

  Scenario: A failed write to the credentials file keeps the rule in memory and informs the developer
    Given a credential card for "github" is visible
    And "~/.lns-credentials.json" cannot be written
    When the developer picks "use from host"
    Then the workload's held request leaves the boundary with the host-detected GitHub credential substituted in
    And the running credential rules contain a "host-detect" entry for "github"
    And the approval window informs the developer that the rule could not be persisted
    And a future request carrying the GitHub placeholder is exchanged silently using the host-detected value until the sandbox exits
