Feature: a connector is offered on first use, whatever the policy already allows
  A connector reaches a workload because the workload reaches its domain, not
  because a file named it first. Nothing in `./lns.yaml` and nothing in
  `lns-policy.yaml` has to mention a connector for it to be offered: the
  machine catalog claims the domain, and the first request there raises the
  connect offer. An allow rule the user already wrote for that domain must not
  swallow the offer, so the launch withholds that rule until the connector is
  decided — the request is held, the offer is answered, and the user's own rule
  then applies as written. A decline is remembered for this workload alone, and
  from then on the rule stands and the request goes out unoffered.

  Scenario: A connector is offered on a domain nothing mentioned
    Given the catalog claims "api.some-provider.example" for connector "some-provider" managing "SOME_TOKEN"
    And the directory policy lists no connectors
    When the workload requests "api.some-provider.example"
    Then the approval window offers to connect "some-provider"

  Scenario: An already-allowed domain still offers the connector
    Given the catalog claims "api.some-provider.example" for connector "some-provider" managing "SOME_TOKEN"
    And the directory policy allows "api.some-provider.example"
    And the directory policy lists no connectors
    When the workload requests "api.some-provider.example"
    Then the approval window offers to connect "some-provider"
    And the request is held until the offer is answered

  Scenario: Answering the offer without connecting is remembered as a decline
    Given the catalog claims "api.some-provider.example" for connector "some-provider" managing "SOME_TOKEN"
    And the directory policy allows "api.some-provider.example"
    And the directory policy lists no connectors
    When the workload requests "api.some-provider.example"
    And the developer answers the card without connecting
    Then the "some-provider" decline is remembered for this workload

  Scenario: A declined offer is not raised again for this workload
    Given the catalog claims "api.some-provider.example" for connector "some-provider" managing "SOME_TOKEN"
    And the directory policy allows "api.some-provider.example"
    And this workload declined the "some-provider" connect offer
    When the workload requests "api.some-provider.example"
    Then no offer is presented
    And the request proceeds under the network policy alone

  Scenario: A connected connector is not offered again
    Given the catalog claims "api.some-provider.example" for connector "some-provider" managing "SOME_TOKEN"
    And the directory policy connects "some-provider"
    When the workload requests "api.some-provider.example"
    Then no offer is presented
    And the request proceeds under the network policy alone

  Scenario: Another workload's decline does not silence this one's offer
    Given the catalog claims "api.some-provider.example" for connector "some-provider" managing "SOME_TOKEN"
    And the directory policy allows "api.some-provider.example"
    And another workload declined the "some-provider" connect offer
    When the workload requests "api.some-provider.example"
    Then the approval window offers to connect "some-provider"

  Scenario: A connector this workload already granted is not withheld or re-offered
    Given the catalog claims "api.some-provider.example" for connector "some-provider" managing "SOME_TOKEN"
    And the directory policy allows "api.some-provider.example"
    And this workload granted the "some-provider" connect offer
    When the workload requests "api.some-provider.example"
    Then no offer is presented
    And the request proceeds under the network policy alone

  Scenario: A connector that shares a connected connector's domain withholds nothing
    Given the catalog claims "api.some-provider.example" for connector "some-provider" managing "SOME_TOKEN"
    And the catalog claims "api.some-provider.example" for connector "other-provider" managing "OTHER_TOKEN"
    And the directory policy connects "some-provider"
    When the workload requests "api.some-provider.example"
    Then no offer is presented
    And the request proceeds under the network policy alone

  Scenario: An unrelated domain's allow rule is left alone
    Given the catalog claims "api.some-provider.example" for connector "some-provider" managing "SOME_TOKEN"
    And the directory policy allows "api.unrelated.example"
    And the directory policy lists no connectors
    When the workload requests "api.unrelated.example"
    Then no offer is presented
    And the request proceeds under the network policy alone
