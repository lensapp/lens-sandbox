Feature: host bind mounts — virtio-fs shares, read-only mode, dropped secrets, audit
  Guest-observable outcomes (the workload reading host files, writes landing back
  on the host, read-only enforcement, a dropped file being absent) need a microVM
  and live in the @microvm e2e contract. These scenarios pin the behaviour the
  service owns without booting a VM: a host bind becomes a tagged virtio-fs share
  in the VM spec (not a block-device disk like a named volume), each bind gets its
  own share tag, read-only is carried on the share, the secret paths the operator
  chose to DROP are threaded onto the spec so the guest can mask them, and the
  bind is recorded in the audit chain.

  Scenario: A host bind is attached as a virtio-fs share at its target, writable
    When a run requests host bind "/Users/me/proj" at "/work"
    Then the spec carries a virtio-fs share for "/Users/me/proj" at "/work"
    And that share is writable

  Scenario: A read-only host bind is marked read-only on its share
    When a run requests host bind "/Users/me/proj" at "/work" read-only
    Then the spec marks the share for "/Users/me/proj" read-only

  Scenario: Two host binds in one run get distinct share tags
    When a run requests host bind "/Users/me/proj" at "/work" and "/Users/me/data" at "/data"
    Then the spec carries two virtio-fs shares with distinct tags
    And the content share tag is left untouched

  Scenario: Dropped secret paths are threaded onto the bind spec for the guest to mask
    When a run requests host bind "/Users/me/proj" at "/work" dropping ".env"
    Then the bind spec for "/work" lists ".env" in its dropped paths

  Scenario: A path a fileset writes into is threaded onto the bind spec for the guest to leave alone
    When a run requests host bind "/Users/me/.claude" at "/root/.claude" seeding "settings.json"
    Then the bind spec for "/root/.claude" lists "settings.json" in its seeded paths

  Scenario: A bind no fileset writes into declares no seeded paths, so the guest mounts it whole
    When a run requests host bind "/Users/me/proj" at "/work"
    Then the bind spec for "/work" declares no seeded paths

  Scenario: Attaching a host bind emits an audit record
    When a host bind "/Users/me/proj" at "/work" is recorded in the audit chain
    Then the audit chain records the host source "/Users/me/proj" and target "/work"

  Scenario: The audit record distinguishes exposed secrets from masked ones
    When a host bind "/Users/me/proj" at "/work" exposing ".env" and dropping ".npmrc" is recorded in the audit chain
    Then the audit chain records ".env" as exposed and ".npmrc" as dropped
