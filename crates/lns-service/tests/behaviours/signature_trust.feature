Feature: a bundle's signature is verified against trusted signers before it runs
  A configured agent is code, so lns refuses to run a tampered or
  unattributed bundle. Signatures are cosign-compatible, attached as OCI
  referrers; signing a bundle transitively vouches for its digest-pinned
  components. Trusted signers are a per-machine key set held in lns config.
  Once any trusted key is configured, verification is enforced for remote
  refs — an unsigned or untrusted-signer bundle is refused. With no key
  configured there is nothing to check against, so lns warns but proceeds.
  --insecure opts out for a single run. (The cryptographic trusted /
  untrusted / unsigned verification itself is pinned by technical units.)

  NOTE: these scenarios pin the gate() DECISION logic across every SignatureStatus.
  Signature VERIFICATION is a tracked follow-up and is not yet wired: the live
  path has no trusted-signer config and attaches/reads no referrer, so it can only
  reach the "no key configured → warn and proceed" and "--insecure → skip" verdicts
  today. The trusted/untrusted/refused verdicts here exercise the logic that
  activates once verification lands.

  Scenario: With a trusted key configured, a bundle signed by that key runs
    Given a trusted signer key is configured
    And a remote bundle signed by that trusted key
    When the bundle is run
    Then verification succeeds
    And the bundle runs

  Scenario: With a trusted key configured, an unsigned remote bundle is refused
    Given a trusted signer key is configured
    And a remote bundle carrying no signature
    When the bundle is run
    Then the run is refused because the bundle is unsigned
    And nothing is launched

  Scenario: With a trusted key configured, a bundle signed by an untrusted key is refused
    Given a trusted signer key is configured
    And a remote bundle signed by a key that is not trusted
    When the bundle is run
    Then the run is refused because the signer is not trusted
    And nothing is launched

  Scenario: With no trusted key configured, an unsigned remote bundle warns but proceeds
    Given no trusted signer key is configured
    And a remote bundle carrying no signature
    When the bundle is run
    Then a warning is surfaced that the signature cannot be verified
    And the bundle runs

  Scenario: --insecure skips verification for a single run
    Given a trusted signer key is configured
    And a remote bundle carrying no signature
    When the bundle is run with --insecure
    Then verification is skipped
    And the bundle runs