Feature: the effective policy in force is disclosed
  What governs a run must never be a mystery. For a bundle, the shipped
  Policy is the baseline and the current directory's lns-policy.yaml is a
  local overlay; for a plain image there is only the cwd file; and --policy
  replaces the baseline entirely. Whatever is in effect, the run reports the
  baseline source and any overlay, so a consumer can see what they are
  running under. These scenarios pin only the disclosure — the deny-dominant
  layering logic itself is a separate concern.

  Scenario: A bundle run names its shipped baseline and the local overlay
    Given a bundle that ships a policy
    And the current directory has an "lns-policy.yaml"
    When the run reports its effective policy
    Then the baseline source is named as the bundle's shipped policy
    And the local overlay is named as the current directory's "lns-policy.yaml"

  Scenario: A plain image run names only the current directory policy
    Given a plain image run
    And the current directory has an "lns-policy.yaml"
    When the run reports its effective policy
    Then the policy source is named as the current directory's "lns-policy.yaml"
    And no bundle baseline is named

  Scenario: --policy replaces the bundle baseline and is disclosed as the source
    Given a bundle that ships a policy
    When the bundle is run with --policy pointing at "/tmp/team-policy.yaml"
    Then the baseline source is named as "/tmp/team-policy.yaml"
    And the bundle's shipped policy is reported as replaced