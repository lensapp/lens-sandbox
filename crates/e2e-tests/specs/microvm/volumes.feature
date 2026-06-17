# Parked under specs/microvm/ (not the globbed features/ dir): these
# scenarios have no step definitions yet, so they are inert documentation of
# the intended volume behaviour until their guest-observable step glue is
# written. Move back under features/ when implemented — make e2e-microvm runs
# every @microvm feature there. See CLAUDE.md "Out of scope".
@microvm
Feature: named volumes persist workload state across runs
  `lns run` starts from a blank, discarded writable layer every time.
  A named volume is host-backed storage, identified by name, that a user
  explicitly attaches to a guest path so a stateful workload's data
  survives between runs. Ephemeral-by-default is preserved: no volume is
  attached unless the user asks for one.

  These scenarios are guest-observable: they require a booted microVM
  with the overlay rootfs, so they are tagged @microvm and run only on a
  virt-capable runner. The host-side store, locking, validation, and
  spec-assembly behaviour has a runnable companion in
  crates/lns-service/tests/behaviours/volumes.feature.

  Scenario: First attach of a new name creates the volume, seeded from the image
    Given no volume named "prism-data" exists
    And the image ships seed files at `/data`
    When the user runs `lns run -v prism-data:/data <image>`
    Then a volume named "prism-data" is created in the global store
    And it is seeded with the image's contents at `/data`
    And it is mounted at `/data` in the guest, writable
    And the run summary lists the attached volume and its guest path

  Scenario: An existing volume is reused as-is, not re-seeded from the image
    Given volume "prism-data" already holds data
    And the image ships different seed files at `/data`
    When a later `lns run -v prism-data:/data <image>` starts
    Then `/data` shows the volume's contents, not the image's seed files

  Scenario: Data written to a volume is there on the next run
    Given a run wrote a file under `/data` with volume "prism-data" attached
    When a later `lns run -v prism-data:/data <image>` starts
    Then the previously written file is present under `/data`

  Scenario: Without -v the run is fully ephemeral (invariant preserved)
    Given a run wrote a file under `/data` with no volume attached
    When a later `lns run <image>` starts with no volume attached
    Then `/data` is empty — nothing from the previous run persists

  Scenario: A volume is name-keyed, not path-keyed
    Given volume "prism-data" holds data written while mounted at `/data`
    When a later run attaches it with `-v prism-data:/srv/state`
    Then the same data appears under `/srv/state`

  Scenario: A read-only attach exposes data but rejects writes
    Given volume "prism-data" holds data
    When a run attaches it with `-v prism-data:/data:ro`
    Then the workload can read `/data`
    And writes under `/data` fail
    And the volume's contents are unchanged after the run

  Scenario: First read-only attach of a new volume is still seeded from the image
    Given no volume named "prism-data" exists
    And the image ships seed files at `/data`
    When the user runs `lns run -v prism-data:/data:ro <image>`
    Then `/data` shows the image's seed files, read-only
    And writes under `/data` fail

  Scenario: The same volume cannot be attached to two live runs at once
    Given a run is live with volume "prism-data" attached
    When a second `lns run -v prism-data:/data <image>` is started
    Then the second run fails with a clear "volume in use by run #N" error
    And the first run is unaffected

  Scenario: An invalid volume name is rejected before the VM boots
    When the user runs `lns run -v "../etc:/data" <image>`
    Then the command fails with a volume-name validation error
    And no VM is booted and no host path outside the store is touched

  Scenario: Attaching a volume is recorded in the audit history
    When a run attaches volume "prism-data" at `/data`
    Then the audit chain records the volume name and guest mount path
