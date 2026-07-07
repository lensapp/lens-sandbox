Feature: the producer build cache and standalone push
  `lns build -t <ref>` assembles an artifact into the local build cache without
  contacting a registry, and a later `lns push <ref>` uploads exactly those
  cached bytes. This runs virt-free (no microVM, no daemon) against an
  in-process registry on loopback, so it is part of the regular e2e suite.

  Scenario: an artifact built into the cache is uploaded by a standalone push
    Given a local registry
    When the user builds a sandbox into the cache and then pushes it from the cache
    Then the registry serves the pushed artifact at its ref
