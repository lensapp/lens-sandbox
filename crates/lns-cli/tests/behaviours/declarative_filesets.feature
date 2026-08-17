Feature: declared filesets ship files inside the sandbox artifact
  spec.filesets is how a sandbox ships files — agent settings, skills —
  inside the published artifact. An entry names either a directory beside
  the document (path — packed into a layer of the same artifact at push,
  so the files and the declaration that mounts them share one digest), a
  small inline UTF-8 file map carried by the document itself, or one file
  read off the machine that runs it (hostPath). At launch the files are
  materialized into the guest at mountPath as a snapshot: a local run of
  a path fileset sees exactly what a consumer of the published artifact
  would see — live files are spec.volumes' job. Trust is one digest plus
  disclosure, not signatures: inspect and the run summary name every
  fileset.

  Scenario: a local run snapshots a path fileset and discloses it
    Given an lns.yaml declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains "prompts.md"
    When the local sandbox run is prepared
    Then the definition sent to the service roots the fileset path under the project
    And the run summary shows a Fileset line `./skills -> /root/.agent/skills`

  Scenario: running a pulled sandbox shows its declared filesets in the summary
    Given a pulled sandbox whose view declares a packed fileset "./skills" mounted at "/root/.agent/skills"
    When the pulled sandbox run is prepared
    Then the run summary shows a Fileset line `./skills -> /root/.agent/skills`

  Scenario: a secret-shaped file in a path fileset refuses the run
    Given an lns.yaml declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains ".env"
    When the user runs the local sandbox
    Then the command fails naming ".env"
    And no run is started

  Scenario: a local run preserves and discloses an inline fileset without printing its content
    Given an lns.yaml declaring an inline fileset with ".claude/settings.json" at "/home/sandbox" owned by the workload
    And the inline file contains `{"marker":"do-not-print"}`
    When the local sandbox run is prepared
    Then the definition sent to the service carries the inline file unchanged
    And the run summary discloses an inline fileset at "/home/sandbox" owned by the workload
    And the run summary does not contain "do-not-print"

  Scenario: a pulled sandbox discloses an inline fileset without printing its content
    Given a pulled sandbox whose view declares an inline fileset at "/home/sandbox" owned by root
    When the pulled sandbox run is prepared
    Then the run summary discloses an inline fileset at "/home/sandbox" owned by root
    And the run summary does not contain the inline file content

  Scenario: a local run discloses a host file source and that it is optional
    Given an lns.yaml declaring a hostPath fileset "~/.gitconfig" mounted at "/home/agent/.gitconfig" and optional
    When the local sandbox run is prepared
    Then the run summary shows a Fileset line `host file ~/.gitconfig (optional) -> /home/agent/.gitconfig`

  Scenario: a pulled sandbox's host file is named in the disclosure before it boots
    Given a pulled sandbox whose view declares a hostPath fileset "~/.gitconfig" at "/home/agent/.gitconfig" and optional
    When the pulled sandbox effects are confirmed with no answer
    Then the disclosure names the host file "host file ~/.gitconfig (optional) → /home/agent/.gitconfig"
    And the run is refused without a confirmation

  Scenario: the disclosure says a host file is read from this machine, not shipped by the author
    Given a pulled sandbox whose view declares a hostPath fileset "~/.gitconfig" at "/home/agent/.gitconfig" and optional
    When the pulled sandbox effects are confirmed with no answer
    Then the disclosure names the host file "read from this machine at launch"
    And the disclosure does not call the host file author-published

  Scenario: the disclosure still calls a packed fileset author-published
    Given a pulled sandbox whose view declares an inline fileset at "/home/sandbox" owned by root
    When the pulled sandbox effects are confirmed with no answer
    Then the disclosure names the host file "author-published files"
