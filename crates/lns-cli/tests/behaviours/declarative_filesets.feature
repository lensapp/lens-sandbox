Feature: declared filesets ship files inside the sandbox artifact
  spec.filesets is how a sandbox ships files — agent settings, skills —
  inside the published artifact. An entry names either a local directory
  (path — packed into a FileSet artifact and digest-pinned by push) or a
  pre-published digest-pinned FileSet (ref). At launch the files are
  materialized into the guest at mountPath as a snapshot: a local run of
  a path fileset sees exactly what a consumer of the published artifact
  would see — live files are spec.volumes' job. Trust is digest pinning
  plus disclosure, not signatures: inspect and the run summary name
  every fileset.

  Scenario: a local run snapshots a path fileset and discloses it
    Given an lns.yaml declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains "prompts.md"
    When the local sandbox run is prepared
    Then the definition sent to the service roots the fileset path under the project
    And the run summary shows a Fileset line `./skills -> /root/.agent/skills`

  Scenario: a local definition may declare a pre-published fileset by ref
    Given an lns.yaml declaring fileset ref "registry.example.test/team/skills@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" mounted at "/root/.agent/skills"
    When the local sandbox run is prepared
    Then the definition sent to the service carries the fileset ref unchanged
    And the run summary shows a Fileset line `registry.example.test/team/skills@sha256:aaaaaaaaaaaa… -> /root/.agent/skills`

  Scenario: a secret-shaped file in a path fileset refuses the run
    Given an lns.yaml declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains ".env"
    When the user runs the local sandbox
    Then the command fails naming ".env"
    And no run is started
