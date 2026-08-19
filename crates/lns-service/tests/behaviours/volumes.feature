Feature: named volumes — host-side store, locking, and attach
  Guest-observable persistence outcomes need a microVM and live in the
  microvm-tagged e2e contract. These scenarios pin the behaviour the
  service owns without booting a VM: the store creates and reuses per-name
  backing images, enforces one-live-attach, validates names before
  touching disk, threads the attach into the VM spec, and records the
  attach in the audit chain.

  Scenario: Attaching an unknown volume name creates its backing image
    Given no volume named "prism-data" exists in the store
    When a run requests volume "prism-data" at "/data"
    Then a backing image for "prism-data" is created in the store

  Scenario: A declared size creates the volume at that size
    Given no volume named "prism-data" exists in the store
    When a run requests volume "prism-data" at "/data" sized 40Gi
    Then the backing image for "prism-data" holds 40Gi

  Scenario: A larger declared size grows the volume that already exists
    Given volume "prism-data" already exists in the store holding 10Gi
    When a run requests volume "prism-data" at "/data" sized 40Gi
    Then no backing image is created
    And the backing image for "prism-data" holds 40Gi

  Scenario: A smaller declared size leaves the volume alone
    Given volume "prism-data" already exists in the store holding 40Gi
    When a run requests volume "prism-data" at "/data" sized 10Gi
    Then the backing image for "prism-data" holds 40Gi

  Scenario: One volume mounted twice takes the larger of the two sizes
    Given no volume named "prism-data" exists in the store
    When a run requests volume "prism-data" at "/data" sized 10Gi and at "/srv" sized 40Gi
    Then the backing image for "prism-data" holds 40Gi

  Scenario: Attaching an existing volume reuses its backing image
    Given volume "prism-data" already exists in the store
    When a run requests volume "prism-data" at "/data"
    Then no backing image is created
    And the spec attaches "prism-data" at "/data"

  Scenario: The VM spec carries the volume at its target path, writable
    When a run requests volume "prism-data" at "/data"
    Then the spec attaches "prism-data" at "/data"
    And that attachment is writable

  Scenario: A read-only attach is marked read-only in the VM spec
    When a run requests volume "prism-data" at "/data" read-only
    Then the spec marks the "prism-data" attachment read-only

  Scenario: The same volume mounted at two paths in one run shares one backing image
    When a run requests volume "prism-data" at both "/data" and "/srv/state"
    Then the spec attaches "prism-data" at both "/data" and "/srv/state"
    And the backing image for "prism-data" is created exactly once

  Scenario: A second live attach of the same volume is refused
    Given a live run holds volume "prism-data"
    When a run requests volume "prism-data" at "/data"
    Then the request is refused because the volume is in use
    And the first run's hold on "prism-data" is unaffected

  Scenario: Releasing a run frees its volume for the next attach
    Given a run held volume "prism-data" and has since ended
    When a run requests volume "prism-data" at "/data"
    Then the request succeeds

  Scenario: An invalid volume name is refused before any image is created
    When a run requests volume "../etc" at "/data"
    Then the request is refused with a volume-name validation error
    And no backing image is created
    And no path outside the store is touched

  Scenario: Attaching a volume emits an audit record
    When a volume "prism-data" at "/data" is recorded in the audit chain
    Then the audit chain records the volume name "prism-data" and target "/data"
