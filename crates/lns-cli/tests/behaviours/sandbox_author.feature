Feature: authoring a document
  A document is authored on disk as `./lns.yaml`, or as whatever file `-f`
  names. `init` scaffolds the kind you ask for, and `validate` and `inspect`
  answer for whichever kind the file declares — offline, either way.

  Scenario: init scaffolds a default sandbox definition with every spec field
    Given the current directory has no lns.yaml
    When the user runs artifact command "init"
    Then the exit code is 0
    And a file "lns.yaml" is created
    And the file "lns.yaml" contains "kind: sandbox"
    And the file "lns.yaml" contains "apiVersion: lns.run/v1"
    And the file "lns.yaml" contains "workdir: /workspace"
    And the file "lns.yaml" contains "volumes:"
    And the file "lns.yaml" contains "env:"
    And the file "lns.yaml" contains "resources:"
    And the file "lns.yaml" contains "egress:"
    And the file "lns.yaml" contains "connectors:"
    And the file "lns.yaml" contains "credentials:"
    And the file "lns.yaml" contains "filesets:"
    And the file "lns.yaml" contains "ports:"
    And the file "lns.yaml" contains "tools: []"

  Scenario: the created-file line lands on stderr, so a piped stdout stays the answer
    Given the current directory has no lns.yaml
    When the user runs artifact command "init"
    Then the exit code is 0
    And the command's stderr contains "✓ created lns.yaml"
    And the command's stdout does not contain "✓"

  Scenario: the scaffolded definition is valid as written
    Given the current directory has no lns.yaml
    When the user runs artifact command "init"
    And the user runs artifact command "validate"
    Then the exit code is 0
    And the output contains "valid"

  Scenario: init refuses to clobber an existing definition
    Given the current directory already has an lns.yaml
    When the user runs artifact command "init"
    Then the command fails with an exit code other than 0
    And the output contains "already exists"
    And the existing lns.yaml is left unchanged

  Scenario: init scaffolds a mixin when asked for one
    Given the current directory has no lns.yaml
    When the user runs artifact command "init --kind mixin"
    Then the exit code is 0
    And a file "lns.yaml" is created
    And the file "lns.yaml" contains "kind: mixin"
    And the file "lns.yaml" does not contain "image:"

  Scenario: the scaffolded mixin is valid as written
    Given the current directory has no lns.yaml
    When the user runs artifact command "init --kind mixin"
    And the user runs artifact command "validate"
    Then the exit code is 0
    And the output contains "valid"

  Scenario: init writes the file -f names
    Given the current directory has no lns.yaml
    When the user runs artifact command "init -f lns.dev.yaml"
    Then the exit code is 0
    And a file "lns.dev.yaml" is created
    And the file "lns.dev.yaml" contains "kind: sandbox"

  Scenario: init refuses to clobber the file -f names
    Given the current directory already has an lns.dev.yaml
    When the user runs artifact command "init -f lns.dev.yaml"
    Then the command fails with an exit code other than 0
    And the output contains "already exists"

  Scenario: init scaffolds no kind it cannot also validate
    When I run "lns init --kind sorcery"
    Then the exit code is 2
    And the output contains "invalid value"

  Scenario: init takes no flag that edits the document
    When I run "lns init --image alpine"
    Then the exit code is 2
    And the output contains "unexpected argument"

  Scenario: validate runs the schema and cross-field checks offline
    Given a valid lns.yaml in the current directory
    When the user runs artifact command "validate"
    Then the exit code is 0
    And the output contains "valid"
    And the service received no request

  Scenario: validate refuses an unknown nested definition field
    Given an lns.yaml with a misspelled volume readOnly field
    When the user runs artifact command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "unknown field"
    And the service received no request

  Scenario: validate answers for a mixin document too
    Given an lns.yaml holding a mixin document
    When the user runs artifact command "validate"
    Then the exit code is 0
    And the output contains "valid"
    And the service received no request

  Scenario: validate --kind holds the document to the kind you named
    Given an lns.yaml holding a mixin document
    When the user runs artifact command "validate --kind sandbox"
    Then the command fails with an exit code other than 0
    And the output contains "is a mixin"
    And the service received no request

  Scenario: validate --kind passes a document that is that kind
    Given an lns.yaml holding a mixin document
    When the user runs artifact command "validate --kind mixin"
    Then the exit code is 0
    And the output contains "valid"
    And the service received no request

  Scenario: validate refuses a mixin that claims a block the sandbox owns
    Given an lns.yaml holding a mixin document that declares an image
    When the user runs artifact command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "a mixin must not declare image"
    And the service received no request

  Scenario: validate refuses a document the other verbs cannot run
    Given an lns.yaml written against the retired lens.dev/v1alpha1 group
    When the user runs artifact command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "lns.run/v1"
    And the service received no request

  Scenario: validate and inspect agree on the same document
    Given an lns.yaml written against the retired lens.dev/v1alpha1 group
    When the user runs artifact command "validate"
    Then the command fails with an exit code other than 0
    When the user runs artifact command "inspect"
    Then the command fails with an exit code other than 0

  Scenario: inspect with no target renders the effective definition offline
    Given a valid lns.yaml in the current directory
    When the user runs artifact command "inspect"
    Then the exit code is 0
    And the output contains "image"
    And the output contains "egress"
    And the service received no request

  Scenario: inspect renders a mixin, not only a sandbox
    Given an lns.yaml holding a mixin document
    When the user runs artifact command "inspect"
    Then the exit code is 0
    And the output contains "postgres-tools"
    And the output contains "node@22"
    And the service received no request

  Scenario: inspect discloses the run-as user a definition asks for
    Given an lns.yaml declaring user "root"
    When the user runs artifact command "inspect"
    Then the exit code is 0
    And the output contains "user:         root"
    And the service received no request

  Scenario: inspect discloses every declared credential and where its value may travel
    Given an lns.yaml declaring the "SOME_TOKEN" credential for "api.some-provider.example"
    When the user runs artifact command "inspect"
    Then the exit code is 0
    And the output contains "credential: SOME_TOKEN -> api.some-provider.example"
    And the service received no request

  Scenario: inspect says so when a declared credential's value travels nowhere
    Given an lns.yaml declaring the "SOME_TOKEN" credential with no destination
    When the user runs artifact command "inspect"
    Then the exit code is 0
    And the output contains "credential: SOME_TOKEN (travels nowhere)"
    And the service received no request

  Scenario: inspect of a path-shaped target renders the definition offline
    Given a valid lns.yaml in the current directory
    When the user runs artifact command "inspect ."
    Then the exit code is 0
    And the output contains "image"
    And the service received no request

  Scenario: there is no standalone show command
    When I run "lns sandbox show"
    Then the exit code is 2
    And the output contains "unrecognized subcommand"

  Scenario: validate accepts a path fileset whose directory exists
    Given an lns.yaml declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains "prompts.md"
    When the user runs artifact command "validate"
    Then the exit code is 0

  Scenario: validate refuses a path fileset whose directory is missing
    Given an lns.yaml declaring fileset "./skills" mounted at "/root/.agent/skills"
    When the user runs artifact command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "./skills"

  Scenario: validate refuses a fileset entry naming no source
    Given an lns.yaml declaring a fileset entry with no source
    When the user runs artifact command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "exactly one of path, inline, or hostPath"

  Scenario: validate refuses a fileset entry naming another artifact
    Given an lns.yaml declaring a fileset entry that names another artifact by ref
    When the user runs artifact command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "unknown field `ref`"

  Scenario: validate refuses a relative fileset guestPath
    Given an lns.yaml declaring fileset "./skills" mounted at "skills"
    When the user runs artifact command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "absolute"

  Scenario: validate refuses a fileset mounted into the sandbox runtime namespace
    Given an lns.yaml declaring fileset "./skills" mounted at "/.lens/bin"
    When the user runs artifact command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "/.lens runtime namespace"

  Scenario: validate refuses a duplicate fileset guestPath or one colliding with a volume target
    Given an lns.yaml declaring two filesets mounted at "/root/.agent/skills"
    When the user runs artifact command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "duplicate"

  Scenario: validate refuses a secret-shaped file inside a path fileset
    Given an lns.yaml declaring fileset "./skills" mounted at "/root/.agent/skills"
    And the project directory "./skills" contains ".env"
    When the user runs artifact command "validate"
    Then the command fails with an exit code other than 0
    And the output contains ".env"

  Scenario: validate accepts a small inline UTF-8 fileset
    Given an lns.yaml declaring an inline fileset with ".claude/settings.json" at "/home/sandbox" owned by the workload
    And the inline file contains `{"permissions":{"defaultMode":"bypassPermissions"}}`
    When the user runs artifact command "validate"
    Then the exit code is 0

  Scenario: validate refuses a fileset that mixes inline content with a path
    Given an lns.yaml declaring a fileset entry with inline content and path
    When the user runs artifact command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "exactly one of path, inline, or hostPath"

  Scenario Outline: validate refuses an unsafe inline file path
    Given an lns.yaml declaring an inline fileset with path "<path>" at "/home/sandbox"
    When the user runs artifact command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "<path>"

    Examples:
      | path                  |
      | /etc/settings.json    |
      | ../settings.json      |
      | .claude/../state.json |

  Scenario: validate refuses a secret-shaped inline file
    Given an lns.yaml declaring an inline fileset with ".env" at "/home/sandbox"
    When the user runs artifact command "validate"
    Then the command fails with an exit code other than 0
    And the output contains ".env"

  Scenario: validate enforces the inline file size limit per file
    Given an lns.yaml declaring two inline files at "/home/sandbox"
    And one inline file is exactly 131072 bytes
    And the other inline file is 131073 bytes
    When the user runs artifact command "validate"
    Then the command fails with an exit code other than 0
    And the output contains "oversized.json"
    And the output contains "use a path fileset"

  Scenario: validate and inspect understand declarative workdir and mounts
    Given an lns.yaml declaring workdir and declarative mounts
    When the user runs artifact command "validate"
    Then the exit code is 0
    When the user runs artifact command "inspect"
    Then the exit code is 0
    And the output contains "/workspace"
    And the output contains "bind ."
    And the output contains "volume some-cache"
