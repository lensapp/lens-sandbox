Feature: distributing a sandbox
  A sandbox is published to and pulled from an OCI registry as a typed
  artifact. Push builds then uploads in one step; there is no standalone
  build. Pull hands the reference to the service, which caches the
  artifact and prefetches its base image (pinned in lns-service).

  Scenario: push builds then uploads in a single step
    Given a valid lns.yaml in the current directory
    And the registry accepts the push
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "built"
    And the output contains "ghcr.io/team/hermes:1.4.0"

  Scenario: a bare push reference publishes to the Lens hub
    Given a valid lns.yaml in the current directory
    And the registry accepts the push
    When the user runs sandbox command "push hchen/claude-code"
    Then the exit code is 0
    And the output contains "hub.lns.run/hchen/claude-code"

  Scenario: a bare pull reference fetches from the Lens hub
    Given the registry serves the sandbox "hub.lns.run/hchen/claude-code"
    When the user runs sandbox command "pull hchen/claude-code"
    Then the exit code is 0
    And the service received a request to pull "hub.lns.run/hchen/claude-code"

  Scenario: a fully-qualified reference is published where it names
    Given a valid lns.yaml in the current directory
    And the registry accepts the push
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "ghcr.io/team/hermes:1.4.0"

  Scenario: there is no standalone build command
    When I run "lns build ."
    Then the exit code is 2
    And the output contains "unrecognized subcommand"

  Scenario: push --dry-run builds everything and uploads nothing
    Given a valid lns.yaml in the current directory
    When the user runs sandbox command "push --dry-run ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "would push ghcr.io/team/hermes:1.4.0@sha256:"
    And the output contains "nothing uploaded"
    And nothing is pushed

  Scenario: push --dry-run previews the layer digest each path fileset would publish under
    Given a valid lns.yaml in the current directory declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains "prompts.md"
    When the user runs sandbox command "push --dry-run ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "would pack fileset ./skills -> sha256:"
    And nothing is pushed

  Scenario: push --dry-run refuses an invalid definition like a real push
    Given a valid lns.yaml in the current directory declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains ".env"
    When the user runs sandbox command "push --dry-run ghcr.io/team/hermes:1.4.0"
    Then the command fails with an exit code other than 0
    And the output contains ".env"
    And nothing is pushed

  Scenario: push fails clearly when the credential lacks write scope
    Given a valid lns.yaml in the current directory
    And the stored credential for the registry lacks push scope
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the command fails with an exit code other than 0
    And the output contains "push scope"
    And the output contains "ghcr.io"

  Scenario: push packs each path fileset into a layer of the same artifact
    Given a valid lns.yaml in the current directory declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains "prompts.md"
    And the registry accepts the push
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the sandbox artifact carries the packed directory as a layer of its own
    And the published sandbox config keeps the fileset path it was authored with

  Scenario: a secret-shaped file in a path fileset refuses the push
    Given a valid lns.yaml in the current directory declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains ".env"
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the command fails with an exit code other than 0
    And the output contains ".env"
    And nothing is pushed

  Scenario: push carries inline files in the document itself with no layer at all
    Given a valid lns.yaml in the current directory declaring an inline fileset at "/home/sandbox"
    And the registry accepts the push
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the artifact carries no packed layer
    And the published sandbox config carries the inline content unchanged

  Scenario: Publishing pins resolved tool versions
    Given a lns.yaml declaring tools ["node@22"]
    And the version index resolves "node@22" to "22.11.0"
    And the registry accepts the push
    When the user runs sandbox command "push ghcr.io/acme/agent:1.0.0"
    Then the published artifact carries the exact resolved versions

  Scenario: push warns about an exact pin the version index does not list
    Given a lns.yaml declaring tools ["node@99.99.99"]
    And the version index does not list "node@99.99.99"
    And the registry accepts the push
    When the user runs sandbox command "push ghcr.io/acme/agent:1.0.0"
    Then the exit code is 0
    And the output contains "warning"
    And the output contains "node@99.99.99"

  Scenario: pull hands the reference to the service and reports the digest
    Given the registry serves the sandbox "ghcr.io/team/hermes:1.4.0"
    When the user runs sandbox command "pull ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "sha256:"
    And the service received a request to pull "ghcr.io/team/hermes:1.4.0"

  Scenario: tag re-refs a cached sandbox
    Given the sandbox "hermes:1.4.0" is cached
    When the user runs sandbox command "tag hermes:1.4.0 hermes:latest"
    Then the exit code is 0
    And the sandbox "hermes:latest" resolves to the same cached artifact

  Scenario: push carries a hostPath fileset verbatim and packs nothing for it
    Given a valid lns.yaml in the current directory declaring a hostPath fileset "~/.gitconfig" mounted at "/home/agent/.gitconfig"
    And the registry accepts the push
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the artifact carries no packed layer
    And the published sandbox config carries the hostPath unchanged

  Scenario: pulling a published mixin caches the graph it layers on and asks nothing
    Given the registry serves the mixin "ghcr.io/acme/obs-tools:2"
    And sandbox input is non-interactive
    When the user runs sandbox command "pull ghcr.io/acme/obs-tools:2"
    Then the exit code is 0
    And the output contains "pulled ghcr.io/acme/obs-tools:2"
    And the output contains "cached 2 mixin(s) it layers on"
    And the output does not contain "installer runs as root"

  Scenario: a mixin the registry gives no digest for refuses the pull
    Given the registry serves a mixin with no digest at "ghcr.io/acme/obs-tools:2"
    When the user runs sandbox command "pull ghcr.io/acme/obs-tools:2"
    Then the exit code is 1
    And the output contains "did not provide a digest"

  Scenario: push publishes a local mixin before the sandbox that names it
    Given an lns.yaml layering on the local mixin "./mixins/pg/"
    And the local mixin at "./mixins/pg/" is named "postgres-tools"
    And the registry accepts the push
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0 --yes"
    Then the exit code is 0
    And the mixin "ghcr.io/team/postgres-tools" was published before the sandbox
    And the mixin "ghcr.io/team/postgres-tools" was published under its own digest as a tag
    And the published sandbox pins mixin "ghcr.io/team/postgres-tools" by digest
    And the output contains "published mixin ./mixins/pg/"

  Scenario: push lists the mixins it would publish and asks before uploading
    Given an lns.yaml layering on the local mixin "./mixins/pg/"
    And the local mixin at "./mixins/pg/" is named "postgres-tools"
    And the registry accepts the push
    And the user will answer "y" to the sandbox prompt
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "./mixins/pg/"
    And the output contains "ghcr.io/team/postgres-tools"
    And the output contains "Continue?"

  Scenario: declining the mixin publication uploads nothing
    Given an lns.yaml layering on the local mixin "./mixins/pg/"
    And the local mixin at "./mixins/pg/" is named "postgres-tools"
    And the registry accepts the push
    And the user will answer "n" to the sandbox prompt
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 1
    And nothing is pushed
    And the output contains "nothing was published"

  Scenario: push refuses to publish a mixin with no terminal to confirm
    Given an lns.yaml layering on the local mixin "./mixins/pg/"
    And the local mixin at "./mixins/pg/" is named "postgres-tools"
    And the registry accepts the push
    And sandbox input is non-interactive
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 1
    And nothing is pushed
    And the output contains "--yes"

  Scenario: a mixin that layers on another local mixin publishes deepest first
    Given an lns.yaml layering on the local mixin "./mixins/outer/"
    And the local mixin at "./mixins/outer/" is named "outer" and layers on "./inner/"
    And the local mixin at "./mixins/outer/inner/" is named "inner"
    And the registry accepts the push
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0 --yes"
    Then the exit code is 0
    And exactly 3 artifact(s) were uploaded
    And the mixin "ghcr.io/team/inner" was published before the sandbox
    And the published sandbox pins mixin "ghcr.io/team/outer" by digest

  Scenario: a sandbox naming no local mixin still publishes one artifact and asks nothing
    Given a valid lns.yaml in the current directory
    And the registry accepts the push
    And sandbox input is non-interactive
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And exactly 1 artifact(s) were uploaded
    And the output does not contain "Continue?"

  Scenario: a push that fails partway says its mixins are safe to re-push
    Given an lns.yaml layering on the local mixin "./mixins/pg/"
    And the local mixin at "./mixins/pg/" is named "postgres-tools"
    And the registry accepts 1 upload(s) then refuses
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0 --yes"
    Then the exit code is 1
    And the output contains "retrying is safe"

  Scenario: push --dry-run previews every artifact it would publish and asks nothing
    Given an lns.yaml layering on the local mixin "./mixins/pg/"
    And the local mixin at "./mixins/pg/" is named "postgres-tools"
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0 --dry-run"
    Then the exit code is 0
    And the output contains "would publish mixin ./mixins/pg/"
    And the output contains "ghcr.io/team/postgres-tools"
    And the output contains "nothing uploaded"
    And the output does not contain "Continue?"
    And nothing is pushed

  Scenario: an unpinned remote mixin still refuses the push
    Given an lns.yaml layering on the local mixin "ghcr.io/team/observability:2"
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0 --yes"
    Then the exit code is 1
    And nothing is pushed
    And the output contains "digest-pinned"

  Scenario: a mixin the registry refuses names the mixin that failed
    Given an lns.yaml layering on the local mixin "./mixins/pg/"
    And the local mixin at "./mixins/pg/" is named "postgres-tools"
    And the registry accepts 0 upload(s) then refuses
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0 --yes"
    Then the exit code is 1
    And the output contains "publishing mixin ./mixins/pg/"
    And the output contains "retrying is safe"
    And the published sandbox was not uploaded

  Scenario: a mixin that fails after another mixin already pinned one does not claim the uploads are unreferenced
    Given an lns.yaml layering on the local mixins "./mixins/outer/" and "./mixins/other/"
    And the local mixin at "./mixins/outer/" is named "outer" and layers on "./inner/"
    And the local mixin at "./mixins/outer/inner/" is named "inner"
    And the local mixin at "./mixins/other/" is named "other"
    And the registry accepts 2 upload(s) then refuses
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0 --yes"
    Then the exit code is 1
    And the output contains "retrying is safe"
    And the output does not contain "no document references"
    And the published sandbox was not uploaded

  Scenario: a sandbox that fails after its mixins landed does not call them unreferenced
    Given an lns.yaml layering on the local mixin "./mixins/outer/"
    And the local mixin at "./mixins/outer/" is named "outer" and layers on "./inner/"
    And the local mixin at "./mixins/outer/inner/" is named "inner"
    And the registry accepts 2 upload(s) then refuses
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0 --yes"
    Then the exit code is 1
    And the output contains "retrying is safe"
    And the output does not contain "unreferenced"

  Scenario: a fuzzy tool a mixin declares makes the dry-run say the digest may differ
    Given an lns.yaml layering on the local mixin "./mixins/pg/"
    And the local mixin at "./mixins/pg/" is named "postgres-tools" and declares tool "node@22"
    When the user runs sandbox command "push ghcr.io/team/hermes:1.4.0 --dry-run"
    Then the exit code is 0
    And the output contains "may differ"
    And nothing is pushed
