Feature: image pull records the cache entry before reporting success
  `lns image pull` exists to create a managed cache entry, so the
  pulled image must be recorded in the index before the command
  reports success. Otherwise `lns image ls` and `lns image rm` would
  not see an image the user was just told was pulled.

  Scenario: A pulled image is recorded so it can be listed and removed
    When image "registry.example.test/some/image:1.0" with layer "sha256:aaa" of 4000 bytes is pulled
    Then the pull succeeds reporting "registry.example.test/some/image:1.0" at 4000 bytes
    And the image record for "registry.example.test/some/image:1.0" remains in the cache

  Scenario: A pull whose index write fails is reported as a failure
    Given the image index cannot be written
    When image "registry.example.test/some/image:1.0" with layer "sha256:aaa" of 4000 bytes is pulled
    Then the pull is refused because the index could not be written
    And the image record for "registry.example.test/some/image:1.0" is gone from the cache
