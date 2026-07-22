Feature: declared filesets ship files inside the sandbox artifact
  spec.filesets is how a sandbox ships files — agent settings, skills —
  inside the published artifact. An entry names either a local directory
  (path — packed into a FileSet artifact and digest-pinned by push), a
  pre-published digest-pinned FileSet (ref), or a small inline UTF-8 file
  map carried by the sandbox definition itself. At launch the files are
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

  Scenario: running a pulled sandbox shows its declared filesets in the summary
    Given a pulled sandbox whose view declares fileset ref "registry.example.test/team/skills@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" mounted at "/root/.agent/skills"
    When the pulled sandbox run is prepared
    Then the run summary shows a Fileset line `registry.example.test/team/skills@sha256:aaaaaaaaaaaa… -> /root/.agent/skills`

  Scenario: a secret-shaped file in a path fileset refuses the run
    Given an lns.yaml declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains ".env"
    When the user runs the local sandbox
    Then the command fails naming ".env"
    And no run is started

  @todo
  Scenario: a local run preserves and discloses an inline fileset without printing its content
    Given an lns.yaml declaring an inline fileset with ".claude/settings.json" at "/home/sandbox" owned by the workload
    And the inline file contains `{"marker":"do-not-print"}`
    When the local sandbox run is prepared
    Then the definition sent to the service carries the inline file unchanged
    And the run summary discloses an inline fileset at "/home/sandbox" owned by the workload
    And the run summary does not contain "do-not-print"

  @todo
  Scenario: a pulled sandbox discloses an inline fileset without printing its content
    Given a pulled sandbox whose view declares an inline fileset at "/home/sandbox" owned by root
    When the pulled sandbox run is prepared
    Then the run summary discloses an inline fileset at "/home/sandbox" owned by root
    And the run summary does not contain the inline file content
