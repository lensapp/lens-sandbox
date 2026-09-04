Feature: a broker DHCP refusal ends the run loudly
  The service owns the run result and its audit chain. It must distinguish a
  guest boot refusal from a workload that started and returned the same code.

  Scenario: the broker refuses an egress run with no DHCP lease
    Given the broker reports that the guest got no DHCP lease
    When the service handles the broker outcome
    Then the run exit reason is "no_dhcp_lease"
    And the audit event kind is "no_dhcp_lease"
