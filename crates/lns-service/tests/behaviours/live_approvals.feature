Feature: Every interface can consume live approvals
  The service publishes complete current snapshots and accepts decisions about
  the exact presentation a client observed, independently of a desktop window.

  Scenario: A client joining after a request sees the pending approval
    Given the service has a live approval for "api.example.com"
    When a client subscribes to live approvals
    Then the client sees the live approval for "api.example.com"

  Scenario: Two clients cannot answer the same presentation twice
    Given the service has a live approval for "api.example.com"
    When a client subscribes to live approvals
    And the client allows the live request once
    Then the decision reaches the waiting run exactly once
    And a second answer to the same presentation is stale
    And a newly connected client sees no live approvals

  Scenario: An expired presentation cannot answer a later request
    Given the service has a live approval for "api.example.com"
    When a client subscribes to live approvals
    And the request ends and another request reuses its identifier
    Then a second answer to the same presentation is stale
    And a newly connected client still sees the replacement request

  Scenario: A disconnected run cannot accept a decision
    Given the service has a live approval for "api.example.com"
    When a client subscribes to live approvals
    And the run stops receiving decisions
    Then submitting the live answer reports the disconnected run
