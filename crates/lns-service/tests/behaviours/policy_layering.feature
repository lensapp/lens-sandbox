Feature: bundle policy layers under a deny-dominant local overlay
  A bundle ships its own Policy as the baseline, and the current
  directory's lns-policy.yaml is a local overlay that is also where
  decisions made during the run persist. Layering can only tighten:
  deny dominates allow across every layer, so a local deny always beats a
  bundle allow. The overlay may add denies freely, but it may add an allow
  only through an explicit runtime approval that is persisted — never
  silently. --policy replaces the baseline entirely, with no merge. The
  runtime gate with defaultVerdict "ask" always backstops.

  Scenario: A local deny beats a bundle allow
    Given a bundle whose policy allows "api.example.test"
    And the current directory's "lns-policy.yaml" denies "api.example.test"
    When the workload requests "api.example.test"
    Then the request is denied

  Scenario: The overlay tightens the baseline by adding a deny
    Given a bundle whose policy allows "api.example.test"
    And the current directory's "lns-policy.yaml" denies "other.example.test"
    When the workload requests "other.example.test"
    Then the request is denied

  Scenario: The overlay cannot silently widen the baseline
    Given a bundle whose policy has no rule for "api.example.test"
    And the current directory's "lns-policy.yaml" has no rule for "api.example.test"
    When the workload requests "api.example.test"
    Then the request is held pending a decision under defaultVerdict "ask"

  Scenario: A runtime approval persists an allow into the local overlay, not the bundle
    Given a bundle whose policy has no rule for "api.example.test"
    And an approval entry is visible for a request to "api.example.test"
    When the developer picks "always allow"
    Then the current directory's "lns-policy.yaml" gains an allow rule for "api.example.test"
    And the bundle's shipped policy is unchanged

  Scenario: --policy replaces the bundle baseline entirely
    Given a bundle whose policy allows "api.example.test"
    When the bundle is run with --policy that has no rule for "api.example.test"
    And the workload requests "api.example.test"
    Then the bundle's allow does not apply
    And the request is held pending a decision under defaultVerdict "ask"