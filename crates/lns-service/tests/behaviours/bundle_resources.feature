Feature: a bundle's sandbox resources govern the run by default
  Resource precedence mirrors policy: the bundle's Sandbox is authoritative
  by default, an explicit --cpus / -m overrides it, then config defaults,
  then built-ins. So what a bundle ships is what runs unless the consumer
  deliberately overrides it.

  Scenario: The bundle's Sandbox resources are used when nothing overrides them
    Given a bundle whose sandbox requests 4 cpus and 2048 MiB
    When the bundle is run with no resource flags
    Then the run is sized at 4 cpus and 2048 MiB

  Scenario: --cpus and -m override the bundle's Sandbox resources
    Given a bundle whose sandbox requests 4 cpus and 2048 MiB
    When the bundle is run with 2 cpus and 1024 MiB
    Then the run is sized at 2 cpus and 1024 MiB