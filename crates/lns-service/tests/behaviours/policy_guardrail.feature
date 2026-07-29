Feature: an over-broad shipped policy is surfaced, never hidden
  A sandbox could ship a policy that quietly opens it — a
  defaultVerdict of "allow", or a broad "*" / CIDR allow. Such a baseline
  is never applied silently: the first-run summary surfaces it prominently
  so a consumer sees the exposure before trusting the sandbox. (The producer
  side only warns on these shapes at build — it does not reject them, so the
  consumer-side surfacing is what keeps an over-broad baseline from hiding.)

  Scenario: A sandbox whose policy defaults to allow is flagged on first run
    Given a sandbox whose policy has defaultVerdict "allow"
    When the first-run summary is produced
    Then the run summary prominently flags the permissive defaultVerdict

  Scenario: A sandbox policy with a wildcard allow is flagged on first run
    Given a sandbox whose policy allows "*"
    When the first-run summary is produced
    Then the run summary prominently flags the wildcard allow

  Scenario: A sandbox policy with a broad CIDR allow is flagged on first run
    Given a sandbox whose policy allows the CIDR "0.0.0.0/0"
    When the first-run summary is produced
    Then the run summary prominently flags the broad CIDR allow

  # A raw-TCP allow is spliced through with no inspection at all, so a broad one is
  # the widest grant a policy file can express — it must not be the single shape the
  # guardrail cannot see.
  Scenario: A sandbox policy with a broad raw-TCP allow is flagged on first run
    Given a sandbox whose policy splices the CIDR "0.0.0.0/0:5432" raw
    When the first-run summary is produced
    Then the run summary prominently flags the broad CIDR allow
