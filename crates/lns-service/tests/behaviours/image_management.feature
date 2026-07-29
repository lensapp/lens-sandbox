Feature: image cache lifecycle — list, remove, prune
  `lns image` manages the pulled-image cache that `lns run` and
  `lns image pull` fill: enumerate what is cached and which run is
  using it, drop one image, or prune everything no running sandbox
  uses. Removing images reclaims the layer blobs that no remaining
  cached image still references.

  Scenario: Listing an empty cache reports no images
    When the images are listed
    Then the image listing is empty

  Scenario: Listing reports a cached image with its size and layer count
    Given image "registry.example.test/some/image:1.0" is cached with layer "sha256:aaa" of 3000 bytes
    And image "registry.example.test/some/image:1.0" also has layer "sha256:bbb" of 1000 bytes
    When the images are listed
    Then the image listing reports "registry.example.test/some/image:1.0" at 4000 bytes across 2 layers

  Scenario: Listing names the run using an image
    Given image "registry.example.test/some/image:1.0" is cached with layer "sha256:aaa" of 3000 bytes
    And a live run uses image "registry.example.test/some/image:1.0"
    When the images are listed
    Then the image listing names "registry.example.test/some/image:1.0" as in use by the holding run

  Scenario: Listing reports an unused image as idle
    Given image "registry.example.test/some/image:1.0" is cached with layer "sha256:aaa" of 3000 bytes
    When the images are listed
    Then the image listing names "registry.example.test/some/image:1.0" as idle

  Scenario: Tagging within one repository creates another reference to the cached sandbox
    Given image "registry.example.test/team/sandbox:1.0" is cached with layer "sha256:aaa" of 3000 bytes
    When image "registry.example.test/team/sandbox:1.0" is tagged as "registry.example.test/team/sandbox:stable"
    Then the image tag succeeds
    And image "registry.example.test/team/sandbox:stable" has the same digest as "registry.example.test/team/sandbox:1.0"

  Scenario: Tagging into another repository is refused
    Given image "registry.example.test/team/sandbox:1.0" is cached with layer "sha256:aaa" of 3000 bytes
    When image "registry.example.test/team/sandbox:1.0" is tagged as "registry.example.test/other/sandbox:stable"
    Then the image tag is refused because cross-repository publication requires a push
    And the image record for "registry.example.test/other/sandbox:stable" is gone from the cache

  Scenario: Tagging into another registry is refused
    Given image "registry.example.test/team/sandbox:1.0" is cached with layer "sha256:aaa" of 3000 bytes
    When image "registry.example.test/team/sandbox:1.0" is tagged as "other.example.test/team/sandbox:stable"
    Then the image tag is refused because cross-repository publication requires a push
    And the image record for "other.example.test/team/sandbox:stable" is gone from the cache

  Scenario: Removing an idle image drops its record and its unshared layers
    Given image "registry.example.test/some/image:1.0" is cached with layer "sha256:aaa" of 3000 bytes
    When image "registry.example.test/some/image:1.0" is removed
    Then the image record for "registry.example.test/some/image:1.0" is gone from the cache
    And layer "sha256:aaa" is gone from the layer cache
    And the removal reports 3000 reclaimed bytes

  Scenario: Removing an image spares layers shared with another cached image
    Given image "registry.example.test/some/image:1.0" is cached with layer "sha256:shared" of 3000 bytes
    And image "registry.example.test/some/image:1.0" also has layer "sha256:doomed" of 1000 bytes
    And image "registry.example.test/other/image:2.0" is cached with layer "sha256:shared" of 3000 bytes
    When image "registry.example.test/some/image:1.0" is removed
    Then layer "sha256:shared" remains in the layer cache
    And layer "sha256:doomed" is gone from the layer cache
    And the removal reports 1000 reclaimed bytes

  Scenario: Removing an image in use by a live run is refused
    Given image "registry.example.test/some/image:1.0" is cached with layer "sha256:aaa" of 3000 bytes
    And a live run uses image "registry.example.test/some/image:1.0"
    When image "registry.example.test/some/image:1.0" is removed
    Then the request is refused because the image is in use
    And the image record for "registry.example.test/some/image:1.0" remains in the cache

  Scenario: Removing an unknown image is refused
    When image "registry.example.test/absent/image:1.0" is removed
    Then the request is refused because there is no such image

  Scenario: Pruning removes idle images and spares the one a run is using
    Given image "registry.example.test/held/image:1.0" is cached with layer "sha256:held" of 3000 bytes
    And image "registry.example.test/idle/image:1.0" is cached with layer "sha256:idle" of 1000 bytes
    And a live run uses image "registry.example.test/held/image:1.0"
    When the images are pruned
    Then the prune removes only image "registry.example.test/idle/image:1.0"
    And layer "sha256:held" remains in the layer cache
    And the image prune reports 1000 reclaimed bytes

  Scenario: Pruning an empty cache removes nothing
    When the images are pruned
    Then the prune removes no images

  Scenario: Pruning reclaims the provisioned tool cache
    Given a provisioned tool cache of 700 bytes
    When the images are pruned
    Then the provisioned tool cache is gone
    And the image prune reports 700 reclaimed bytes

  Scenario: Pruning preserves shared tool content while a sandbox is live
    Given image "registry.example.test/team/live:1.0" is cached with layer "sha256:live" of 1000 bytes
    And a live run uses image "registry.example.test/team/live:1.0"
    And a provisioned tool cache of 700 bytes
    When the images are pruned
    Then the provisioned tool cache remains
    And the image prune reports 0 reclaimed bytes
