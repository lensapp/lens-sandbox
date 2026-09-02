Feature: lns run mounts host directories into the sandbox (docker `-v host:guest`)
  The day-one need is "run an agent against *this* repo". `lns run -v` gains a
  second flavor alongside named volumes: when the source segment is an absolute
  host path it is a host bind, mounting live host files into the guest; when it
  is a bare name it stays a named volume (the existing behavior, unchanged). The
  hard part is secrets — a bind drags `.env`, keys, and credential files onto the
  guest, silently defeating "real secrets stay outside the workload". So before
  the run starts, `lns` detects secret-shaped files in each bind and, the same
  trust-on-first-use way the network policy works, asks the operator to KEEP or
  DROP each one and remembers the decision. These scenarios pin the CLI grammar,
  the disambiguation rule, the secret decision flow, and the run-summary surface
  against the in-process `Cli`; the live host↔guest byte-path (the guest actually
  seeing the files, writes propagating back, read-only enforcement) is a microVM
  concern covered by the @microvm e2e contract, out of scope here.

  Scenario: An absolute -v source resolves to a host bind
    When the mounts are resolved for `lns run -v /Users/me/proj:/work alpine`
    Then the resolved host binds are exactly "/Users/me/proj -> /work"
    And the resolved volumes are exactly ""

  Scenario: A read-only host bind keeps its :ro marker
    When the mounts are resolved for `lns run -v /Users/me/proj:/work:ro alpine`
    Then the resolved host binds are exactly "/Users/me/proj -> /work:ro"

  Scenario: A bare -v name still resolves to a named volume
    When the mounts are resolved for `lns run -v build-cache:/cache alpine`
    Then the resolved volumes are exactly "build-cache:/cache"
    And the resolved host binds are exactly ""

  Scenario: A host bind target must be absolute with no '..' segments
    When I run "lns run -v /Users/me/proj:work alpine"
    Then the exit code is 2
    And the output contains "must be an absolute path"

  Scenario: A non-existent host source path is refused before the run starts
    Given the host path "/Users/me/typo" does not exist
    When the user runs `lns run -v /Users/me/typo:/work alpine` interactively
    Then the command fails with "host path does not exist"

  Scenario: A single-file host source is refused before the run starts
    Given the host path "/Users/me/.gitconfig" is a file, not a directory
    When the user runs `lns run -v /Users/me/.gitconfig:/etc/gitconfig:ro alpine` interactively
    Then the command fails with "must be a directory"

  Scenario: A clean bind with no secret-shaped files runs without prompting
    Given the host directory "/Users/me/proj" contains no secret-shaped files
    When the user runs `lns run -v /Users/me/proj:/work alpine` interactively
    Then no KEEP or DROP prompt is shown
    And the run starts

  Scenario: First run prompts KEEP or DROP for a detected secret-shaped file
    Given the host directory "/Users/me/proj" contains ".env"
    And no prior decision is recorded for "/Users/me/proj/.env"
    When the user runs `lns run -v /Users/me/proj:/work alpine` interactively
    Then the operator is prompted to KEEP or DROP "/Users/me/proj/.env"

  Scenario: KEEP exposes the real file and records a per-machine decision
    Given the host directory "/Users/me/proj" contains ".env"
    And the operator will answer the secret prompt with "keep"
    When the user runs `lns run -v /Users/me/proj:/work alpine` interactively
    Then ".env" is exposed to the guest under "/work"
    And a per-machine KEEP decision is recorded for "/Users/me/proj/.env"
    And a later run with the same bind shows no prompt

  Scenario: DROP hides the file from the guest and records the decision
    Given the host directory "/Users/me/proj" contains ".env"
    And the operator will answer the secret prompt with "drop"
    When the user runs `lns run -v /Users/me/proj:/work alpine` interactively
    Then ".env" is dropped from the bind
    And a per-machine DROP decision is recorded for "/Users/me/proj/.env"
    And a later run with the same bind shows no prompt

  Scenario: A newly-appeared, undecided secret prompts only for itself
    Given the host directory "/Users/me/proj" contains ".env" and ".npmrc"
    And a per-machine KEEP decision is recorded for "/Users/me/proj/.env"
    And no prior decision is recorded for "/Users/me/proj/.npmrc"
    When the user runs `lns run -v /Users/me/proj:/work alpine` interactively
    Then the operator is prompted only for "/Users/me/proj/.npmrc"

  Scenario: A non-interactive run defaults undecided secrets to DROP
    Given the host directory "/Users/me/proj" contains an undecided ".env"
    When the user runs `lns run -v /Users/me/proj:/work alpine` with no terminal
    Then ".env" is dropped from the bind
    And the dropped path "/Users/me/proj/.env" is reported on stderr
    And no KEEP or DROP prompt is shown

  Scenario: The run summary lists each bind, its mode, and secret disposition
    Given the host directory "/Users/me/proj" contains ".env"
    And the operator will answer the secret prompt with "keep"
    When the user runs `lns run -v /Users/me/proj:/work alpine` interactively
    Then the summary shows a bind line "/Users/me/proj → /work (read-write)"
    And the summary shows ".env: kept (exposed)"

  Scenario: A home-rooted declared bind resolves against this machine's home
    Given a definition declaring a bind from "~/.claude" to "/home/agent/.claude"
    And this machine's home directory is "/home/some-user"
    When the declared mounts resolve
    Then the host bind source is "/home/some-user/.claude"

  Scenario: A project-relative declared bind still resolves against the project directory
    Given a definition declaring a bind from "./src" to "/work"
    And this machine's home directory is "/home/some-user"
    When the declared mounts resolve
    Then the host bind source is "/work/project/src"

  Scenario: A home-rooted declared bind on a machine with no home is refused
    Given a definition declaring a bind from "~/.claude" to "/home/agent/.claude"
    And this machine has no home directory
    When the declared mounts resolve
    Then the mount resolution fails naming "~/.claude"

  Scenario: An optional bind whose host path is missing is skipped and the run continues
    Given an optional declared bind from "/Users/me/.claude" to "/home/agent/.claude"
    And the host path "/Users/me/.claude" does not exist
    When the declared binds are resolved interactively
    Then no host bind is resolved
    And the output says the bind was skipped because it is not present on this host

  Scenario: A required bind whose host path is missing still fails the run
    Given a required declared bind from "/Users/me/.claude" to "/home/agent/.claude"
    And the host path "/Users/me/.claude" does not exist
    When the declared binds are resolved interactively
    Then the command fails with "host path does not exist"

  Scenario: An optional bind whose host path is present is mounted like any other
    Given an optional declared bind from "/Users/me/.claude" to "/home/agent/.claude"
    And the host directory "/Users/me/.claude" contains no secret-shaped files
    When the declared binds are resolved interactively
    Then the resolved host binds are exactly "/Users/me/.claude -> /home/agent/.claude"
