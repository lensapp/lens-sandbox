Feature: lns-service approval flow
  When a workload running in the sandbox takes an action with no
  matching rule, lns-service surfaces the request as a card in the
  always-on-top approval window — a native egui surface hosted by the
  tray-resident daemon, pinned to the top-right of the developer's
  screen. The developer's decision (allow once / always allow / deny
  once / always deny) is delivered straight from the card to the
  running policy. An "always" decision is persisted to the policy
  file used by the run and hot-swapped into the running policy so
  future identical requests are not asked about again. Dismissing a
  card is not a decision: the request fails closed, nothing is
  recorded, and the next one asks again. Policy edits made by the
  developer directly on the file are also hot-swapped.

  Scenario: A workload action with no matching rule prompts the developer
    Given a workload is running in the sandbox
    And the policy has no rule for "api.linear.app"
    When the workload makes a request to "api.linear.app"
    Then an entry appears in the approval window showing the destination, the originating sandbox, and any credential involved
    And the workload's request is held pending a decision

  Scenario: Allow once resolves a single request without changing policy
    Given an approval entry is visible for a request to "api.linear.app"
    When the developer picks "allow once"
    Then the workload's request proceeds
    And the running policy is unchanged
    And the policy file is unchanged
    And a future request to "api.linear.app" prompts again

  Scenario: Always allow writes a rule to the policy file and hot-swaps the running policy
    Given the sandbox was launched with --policy "lns-policy.yaml"
    And the policy has no rule for "api.linear.app"
    And an approval entry is visible for a request to "api.linear.app"
    When the developer picks "always allow"
    Then "lns-policy.yaml" contains a new allow rule for "api.linear.app"
    And the running policy contains the same rule
    And the workload's request proceeds
    And a future request to "api.linear.app" is allowed without prompting

  Scenario: Deny once fails the request without changing policy
    Given an approval entry is visible for a request to "api.linear.app"
    When the developer picks "deny once"
    Then the workload's request is denied
    And the running policy is unchanged
    And the policy file is unchanged
    And a future request to "api.linear.app" prompts again

  Scenario: Closing a network card fails the request without recording a decision
    Given an approval entry is visible for a request to "api.linear.app"
    When the developer closes the card without choosing
    Then the workload's request is denied
    And the running policy is unchanged
    And the policy file is unchanged
    And the audit chain records no approval for "api.linear.app"
    And a future request to "api.linear.app" prompts again

  Scenario: Always deny writes a deny rule to the policy file and hot-swaps the running policy
    Given the sandbox was launched with --policy "lns-policy.yaml"
    And the policy has no rule for "api.linear.app"
    And an approval entry is visible for a request to "api.linear.app"
    When the developer picks "always deny"
    Then "lns-policy.yaml" contains a new deny rule for "api.linear.app"
    And the running policy contains the same rule
    And the workload's request is denied
    And a future request to "api.linear.app" is denied without prompting

  Scenario: Repeated requests to the same destination share one entry
    Given a workload makes a request to "api.linear.app" with no matching rule
    And an approval entry is visible
    When the workload makes a second request to "api.linear.app" before the developer decides
    Then no second card appears in the approval window
    And when the developer's decision is recorded, both requests resolve under that decision

  Scenario: An approval entry with no decision after the timeout fails closed
    Given an approval entry is visible for a request to "api.linear.app"
    When no decision is recorded before the configured approval timeout
    Then the workload's request is denied
    And the approval card is removed from the approval window
    And the running policy is unchanged
    And the policy file is unchanged

  Scenario: A workload exit withdraws its open entries
    Given a workload has an open approval entry
    When the workload exits before a decision is recorded
    Then the approval card is removed from the approval window
    And the running policy is unchanged
    And the policy file is unchanged

  Scenario: A decision-driven policy update is applied to the next request
    Given a workload is running with the loaded policy
    When an "always allow" decision adds a rule for "api.linear.app" mid-run
    Then a subsequent request from the workload to "api.linear.app" is allowed without prompting
    And no restart of the workload is required

  Scenario: A manual edit to the policy file hot-swaps the running policy
    Given a workload is running with "lns-policy.yaml" loaded
    When the developer edits "lns-policy.yaml" to add an allow rule for "api.linear.app"
    Then a subsequent request from the workload to "api.linear.app" is allowed without prompting
    And no restart of the workload is required

  Scenario: A failed policy-file write keeps the rule in memory and notifies the developer
    Given an approval entry is visible for a request to "api.linear.app"
    And the policy file cannot be written
    When the developer picks "always allow"
    Then the workload's request proceeds
    And the running policy contains the new allow rule
    And the approval window informs the developer that the rule could not be persisted
    And a future request to "api.linear.app" is allowed without prompting until the sandbox exits
