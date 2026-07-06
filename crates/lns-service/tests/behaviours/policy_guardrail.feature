@todo
Feature: an over-broad shipped policy is surfaced, never hidden
  A bundle could ship a Policy that quietly opens the sandbox — a
  defaultVerdict of "allow", or a broad "*" / CIDR allow. Such a baseline
  is never applied silently: the first-run summary surfaces it prominently
  so a consumer sees the exposure before trusting the bundle. (The producer
  side only warns on these shapes at build — it does not reject them, so the
  consumer-side surfacing is what keeps an over-broad baseline from hiding.)

  Scenario: A bundle whose policy defaults to allow is flagged on first run
    Given a bundle whose policy has defaultVerdict "allow"
    When the bundle is run
    Then the run summary prominently flags the permissive defaultVerdict

  Scenario: A bundle policy with a wildcard allow is flagged on first run
    Given a bundle whose policy allows "*"
    When the bundle is run
    Then the run summary prominently flags the wildcard allow

  Scenario: A bundle policy with a broad CIDR allow is flagged on first run
    Given a bundle whose policy allows the CIDR "0.0.0.0/0"
    When the bundle is run
    Then the run summary prominently flags the broad CIDR allow