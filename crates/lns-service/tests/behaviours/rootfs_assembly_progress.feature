Feature: rootfs assembly reports determinate progress
  First-run `lns run` of a large image assembles the rootfs from its
  layers, which can take minutes. The assembly must report how many
  layer bytes it has applied out of the total so the CLI can draw the
  same determinate bar the pull phase already shows — an indeterminate
  spinner reads as a hang.

  Scenario: Assembling a multi-layer rootfs reports cumulative bytes per layer
    Given an uncached image with layers of 3072 and 5120 bytes
    When the rootfs is assembled with a recording progress sink
    Then the sink first observes 0 of 8192 bytes
    And the sink observes 3072 of 8192 bytes after the first layer
    And the sink observes 8192 of 8192 bytes after the last layer

  @todo
  Scenario: A cached descriptor is served without any assembly progress
    Given an image whose descriptor was already assembled
    When the same rootfs is requested again with a recording progress sink
    Then the descriptor is served from cache
    And the sink observes no progress at all
