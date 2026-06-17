@microvm
Feature: a real OCI image is pulled and booted end to end
  This closes the gap the cache-management features leave open: they only
  exercise an empty cache, never a successful pull. Here a real image is
  pulled from the registry, then booted so its filesystem is the workload's
  root — exercising resolve, layer download, the content/layer caches, and
  the composefs overlay assembly against a booted guest. These scenarios
  reach the network (Docker Hub) and so, like all @microvm work, run only
  via `make e2e-microvm`, never in CI. The release marker is shell-computed
  so it matches only real workload output, not the echoed image reference.

  Scenario: a pulled image's filesystem is the workload root
    Given the Lens Sandbox service is running
    When the user runs image "alpine:3.20" with command "/bin/sh -c 'echo rel=$(cat /etc/alpine-release)'"
    Then the exit code is 0
    And the output contains "rel=3.20"

  Scenario: pulling records the image so the cache can list it
    Given the Lens Sandbox service is running
    When I run "image pull alpine:3.20"
    Then the exit code is 0
    And the output contains "Pulled"
    When I run "image ls"
    Then the exit code is 0
    And the output contains "alpine"
