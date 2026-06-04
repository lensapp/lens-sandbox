Feature: declaring custom credential providers from the CLI
  Today only the built-in providers (github, openai, anthropic, linear,
  telegram) ship as credential placeholders. Developers need their own
  — a corporate API, an internal service — so the CLI can declare a
  custom provider into `lns-policy.yaml`: an id, an env var, a
  self-identifying placeholder, and one or more per-domain injections.
  These declarations live in the shareable policy file (not the
  per-machine credential store) so a team can commit them alongside
  their project. Declarations carry no resolved credential value — the
  real value is still decided per machine and stored in
  `~/.lns-credentials.json`. The two declarable injection kinds mirror
  the built-in manifest: `bearer_header` and `uri_placeholder`. The
  `awsSigv4` kind carries real STS material and is not declarable here.
  Built-in providers cannot be shadowed or removed.

  Scenario: Declaring a custom provider with a bearer_header injection
    When the developer declares a custom provider "acme" with env var "ACME_API_KEY", placeholder "acme_LNSPLACEHOLDER0000000000000000000000", injection kind "bearer_header", domain "api.acme.corp"
    Then "lns-policy.yaml" contains the "acme" custom provider declaration
    And the declaration carries no resolved credential value

  Scenario: Declaring a custom provider with a uri_placeholder injection
    When the developer declares a custom provider "rocket" with env var "ROCKET_BOT_TOKEN", placeholder "0000000000:LNSPLACEHOLDER000000000000000000000", injection kind "uri_placeholder", domain "api.rocket.example"
    Then "lns-policy.yaml" contains the "rocket" custom provider declaration

  Scenario: A custom provider declared without a placeholder gets a self-identifying generated one
    When the developer declares a custom provider "acme" with env var "ACME_API_KEY" and no placeholder, injection kind "bearer_header", domain "api.acme.corp"
    Then "lns-policy.yaml" contains the "acme" custom provider declaration
    And the "acme" placeholder self-identifies as a placeholder

  Scenario: Declaring a custom provider with a value stores it separately from the declaration
    When the developer declares a custom provider "acme" with env var "ACME_API_KEY", injection kind "bearer_header", domain "api.acme.corp", and value "acme_real"
    Then "lns-policy.yaml" contains the "acme" custom provider declaration
    And the declaration carries no resolved credential value
    And "~/.lns-credentials.json" gains an entry for "acme" with kind "stored" carrying "acme_real"

  Scenario: Declaring a custom provider with a value piped on stdin keeps it off the command line
    When the developer declares a custom provider "acme" with env var "ACME_API_KEY", injection kind "bearer_header", domain "api.acme.corp", and a value piped on stdin as "acme_real"
    Then "lns-policy.yaml" contains the "acme" custom provider declaration
    And the declaration carries no resolved credential value
    And "~/.lns-credentials.json" gains an entry for "acme" with kind "stored" carrying "acme_real"

  Scenario: awsSigv4 cannot be declared from the CLI
    When the developer tries to declare a custom provider with injection kind "awsSigv4"
    Then the command fails with a clear error explaining that awsSigv4 is not declarable from the CLI
    And "lns-policy.yaml" is unchanged

  Scenario: Declaring with both --value and --value-stdin is rejected
    When the developer tries to declare a custom provider passing both --value and --value-stdin
    Then the command is rejected for passing two value sources
    And "lns-policy.yaml" is unchanged

  Scenario: A custom provider can declare multiple domain injections
    Given the developer has declared a custom provider "acme" with a bearer_header injection for "api.acme.corp"
    When the developer adds a second bearer_header injection for "api-eu.acme.corp" to "acme"
    Then "lns-policy.yaml" shows the "acme" provider with two injection entries
    And the command output states that the new injection will not reach a running workload until a new sandbox is launched

  Scenario: Declaring a custom provider whose id collides with a built-in is rejected
    When the developer tries to declare a custom provider with id "github"
    Then the command fails with a clear error
    And "lns-policy.yaml" is unchanged

  Scenario: Declaring a custom provider whose id collides with another custom entry is rejected
    Given "lns-policy.yaml" already declares a custom provider "acme"
    When the developer tries to declare another custom provider with id "acme"
    Then the command fails with a clear error
    And the existing "acme" declaration is unchanged

  Scenario: Declaring a custom provider whose placeholder does not self-identify is rejected
    When the developer tries to declare a custom provider with placeholder "acme_real_looking_token"
    Then the command fails with a clear error
    And "lns-policy.yaml" is unchanged

  Scenario: Listing providers shows both built-in and custom with their source
    Given "lns-policy.yaml" declares a custom provider "acme"
    When the developer lists providers
    Then the output shows the five built-in providers labelled as built-in
    And the "acme" provider labelled as custom from the policy file

  Scenario: Removing a custom provider deletes it from the policy file
    Given "lns-policy.yaml" declares a custom provider "acme"
    When the developer removes the "acme" custom provider
    Then "lns-policy.yaml" no longer contains the "acme" declaration

  Scenario: Removing a built-in provider is rejected
    When the developer tries to remove the built-in "github" provider
    Then the command fails with a clear error
    And "lns-policy.yaml" is unchanged

  Scenario: Declaring a custom provider while a sandbox is running tells the developer a relaunch is required
    Given a sandbox is running
    When the developer declares a new custom provider "acme"
    Then the command succeeds
    And the command output states that the new placeholder will not appear in the running workload's environment until a new sandbox is launched
