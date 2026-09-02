Feature: volume lifecycle — list, create, inspect, remove, prune
  `lns volume` manages the named-volume store that `lns run -v` mounts
  from: enumerate what exists and who holds it, provision a volume ahead
  of its first run, and reclaim space — never touching a volume a live
  run is using.

  Scenario: Listing an empty store reports no volumes
    When the volumes are listed
    Then the listing is empty

  Scenario: Listing reports a volume held by a live run with its holder
    Given a live run holds volume "prism-data"
    When the volumes are listed
    Then the listing names "prism-data" as in use by the holding run

  Scenario: Listing reports an unattached volume as idle
    Given volume "prism-data" already exists in the store
    When the volumes are listed
    Then the listing names "prism-data" as idle

  Scenario: Creating a new volume provisions its backing image
    When volume "prism-data" is created
    Then a backing image for "prism-data" is created in the store

  Scenario: Creating an existing volume succeeds and keeps its backing image
    Given volume "prism-data" already exists in the store
    When volume "prism-data" is created
    Then the operation succeeds
    And no backing image is created

  Scenario: Creating a volume with an invalid name is refused
    When volume "../etc" is created
    Then the request is refused with a volume-name validation error
    And no backing image is created

  Scenario: Inspecting an idle volume reports its size and idle state
    Given volume "prism-data" already exists in the store
    When volume "prism-data" is inspected
    Then the inspection reports "prism-data" as idle
    And the inspection reports the volume's size and disk usage

  Scenario: Inspecting a volume held by a live run names the holder
    Given a live run holds volume "prism-data"
    When volume "prism-data" is inspected
    Then the inspection reports "prism-data" as in use by the holding run

  Scenario: Inspecting an unknown volume is refused
    When volume "prism-data" is inspected
    Then the request is refused because there is no such volume

  Scenario: Removing an idle volume deletes its backing image
    Given volume "prism-data" already exists in the store
    When volume "prism-data" is removed
    Then the backing image for "prism-data" is gone from the store

  Scenario: Removing a volume held by a live run is refused, naming the sandbox
    Given the live sandbox "reviewer" holds volume "prism-data"
    When volume "prism-data" is removed
    Then the request is refused because the volume is in use
    And the refusal names the sandbox "reviewer"
    And the backing image for "prism-data" remains in the store

  Scenario: Removing a volume a stopped sandbox declares is refused
    Given the stopped sandbox "reviewer" declares volume "prism-data"
    When volume "prism-data" is removed
    Then the request is refused because the volume is in use
    And the refusal names the sandbox "reviewer"
    And the backing image for "prism-data" remains in the store

  Scenario: Removing a volume two sandboxes declare names both of them
    Given the stopped sandbox "reviewer" declares volume "prism-data"
    And the stopped sandbox "auditor" declares volume "prism-data"
    When volume "prism-data" is removed
    Then the request is refused because the volume is in use
    And the refusal names the sandbox "reviewer"
    And the refusal names the sandbox "auditor"
    And the backing image for "prism-data" remains in the store

  Scenario: A volume a run declares in a record this build will not run is still held
    Given run "aa07" declares volume "prism-data" in a record this build will not run
    When volume "prism-data" is removed
    Then the request is refused
    And the refusal tells the user to repair run "aa07" and restart the service
    And the refusal does not tell the user to remove a sandbox
    And the backing image for "prism-data" remains in the store

  Scenario: A volume is not removed while a run's claims cannot be read at all
    Given the record of run "aa07" cannot be read
    When volume "prism-data" is removed
    Then the request is refused
    And the refusal tells the user to repair run "aa07" and restart the service

  Scenario: Pruning keeps every volume while a run's claims cannot be read, and says why
    Given volume "scratch" already exists in the store
    And the record of run "aa07" cannot be read
    When the volumes are pruned
    Then the prune removes nothing
    And the prune reports "scratch" as failed, naming run "aa07"
    And the backing image for "scratch" remains in the store

  Scenario: A run whose record cannot be read is not listed as a volume's holder
    Given volume "scratch" already exists in the store
    And the record of run "aa07" cannot be read
    When the volumes are listed
    Then the listing names "scratch" as idle

  Scenario: A volume is removable once the last sandbox declaring it is gone
    Given the stopped sandbox "reviewer" declares volume "prism-data"
    And the sandbox "reviewer" is removed
    When volume "prism-data" is removed
    Then the backing image for "prism-data" is gone from the store

  Scenario: Removing an unknown volume is refused
    When volume "prism-data" is removed
    Then the request is refused because there is no such volume

  Scenario: A removed volume's name is immediately free for re-creation
    Given volume "prism-data" already exists in the store
    When volume "prism-data" is removed
    And volume "prism-data" is created
    Then a backing image for "prism-data" is created in the store

  Scenario: Pruning removes idle volumes and reports the space reclaimed
    Given volume "prism-data" already exists in the store
    And volume "scratch" already exists in the store
    When the volumes are pruned
    Then the prune removes "prism-data" and "scratch"
    And the prune reports the reclaimed space

  Scenario: Pruning skips volumes held by live runs
    Given a live run holds volume "prism-data"
    And volume "scratch" already exists in the store
    When the volumes are pruned
    Then the prune removes only "scratch"
    And the backing image for "prism-data" remains in the store

  Scenario: Pruning skips a volume a stopped sandbox declares
    Given the stopped sandbox "reviewer" declares volume "prism-data"
    And volume "scratch" already exists in the store
    When the volumes are pruned
    Then the prune removes only "scratch"
    And the backing image for "prism-data" remains in the store

  Scenario: Listing names every sandbox that declares a volume
    Given the stopped sandbox "reviewer" declares volume "prism-data"
    And the stopped sandbox "auditor" declares volume "prism-data"
    When the volumes are listed
    Then the listing names "prism-data" as in use by "reviewer" and "auditor"

  Scenario: A dry run names what a prune would remove and removes none of it
    Given volume "scratch" already exists in the store
    And the stopped sandbox "reviewer" declares volume "prism-data"
    When the volumes are pruned as a dry run
    Then the prune removes only "scratch"
    And the backing image for "scratch" remains in the store
    And the backing image for "prism-data" remains in the store

  Scenario: A dry run reports a repair-blocked volume the way the prune itself would
    Given volume "scratch" already exists in the store
    And the record of run "aa07" cannot be read
    When the volumes are pruned as a dry run
    Then the prune removes nothing
    And the prune reports "scratch" as failed, naming run "aa07"

  Scenario: Pruning an empty store removes nothing
    When the volumes are pruned
    Then the prune removes nothing
