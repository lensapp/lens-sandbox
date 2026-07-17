# Parked under specs/microvm/ (not the globbed features/ dir): these three
# scenarios test that a fresh volume is SEEDED from the image's contents at the
# mount path, which an imageless run cannot exercise. They have no step glue
# yet and stay inert until a local-image path exists. The image cache is
# registry-fed today (`lns pull` / `lns run <ref>` resolve refs through a
# registry), so there is no hermetic way to supply an image with /data seed
# files; the clean enabler is a first-class local import (a `docker load`
# equivalent). The non-seeding volume behaviour is covered
# imageless in features/volumes.feature. See CLAUDE.md "Out of scope".
@microvm
Feature: a fresh volume is seeded from the image at the mount path

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

  Scenario: First read-only attach of a new volume is still seeded from the image
    Given no volume named "prism-data" exists
    And the image ships seed files at `/data`
    When the user runs `lns run -v prism-data:/data:ro <image>`
    Then `/data` shows the image's seed files, read-only
    And writes under `/data` fail
