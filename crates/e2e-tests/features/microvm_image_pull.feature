@microvm
Feature: a real OCI image is pulled and booted end to end
  This closes the gap the cache-management features leave open: they only
  exercise a local registry, never a public one. Here a real image is
  pulled from the registry, then booted (through a published sandbox that
  wraps it, since a plain image REF is refused) so its filesystem is the
  workload's root — exercising resolve, layer download, the content/layer caches, and
  the composefs overlay assembly against a booted guest. These scenarios
  reach the network (a public registry) and so, like all @microvm work, run only
  via `make e2e-microvm`, never in CI. The release marker is shell-computed
  so it matches only real workload output, not the echoed image reference.

  Scenario: a pulled image's filesystem is the workload root
    Given the Lens Sandbox service is running
    When the user runs image "public.ecr.aws/docker/library/alpine:3.20" with command "/bin/sh -c 'echo rel=$(cat /etc/alpine-release)'"
    Then the exit code is 0
    And the output contains "rel=3.20"

  Scenario: pulling a plain OCI image from a real registry is refused
    Given the Lens Sandbox service is running
    When I run lns "pull public.ecr.aws/docker/library/alpine:3.20" against the service
    Then the exit code is non-zero
    And the output contains "not a Lens Sandbox artifact"
