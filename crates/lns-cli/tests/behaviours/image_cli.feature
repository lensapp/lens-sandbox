Feature: managing cached images from the CLI
  `lns image` is the lifecycle surface for the pulled-image cache that
  `lns run` boots from: pre-warm an image with pull, list what is
  cached, and reclaim space with rm/prune. The cache lives in the
  service, so every verb is a thin IPC call. Pull prints the resolved
  digest and a copy-pastable pinned reference so reruns can be
  locked to exactly the bytes that were vetted.

  Scenario: the image family lists its verbs in help
    When I run "lns image --help"
    Then the exit code is 0
    And the output contains "ls"
    And the output contains "rm"
    And the output contains "prune"

  Scenario: a bare image invocation surfaces usage
    When I run "lns image"
    Then the exit code is 2
    And the output contains "Usage: lns image"

  Scenario: listing images renders a table with digest, size, and user
    Given the service reports a cached image "registry.example.test/some-image:1.0" of 3145728 bytes used by run 7
    When the user runs image command "ls"
    Then the exit code is 0
    And the output contains "REFERENCE"
    And the output contains "DIGEST"
    And the output contains "registry.example.test/some-image:1.0"
    And the output contains "3 MiB"
    And the output contains "run 000000070000"

  Scenario: listing marks an unused image as idle
    Given the service reports an unused cached image "registry.example.test/some-image:1.0" of 3145728 bytes
    When the user runs image command "ls"
    Then the exit code is 0
    And the listed image row for "registry.example.test/some-image:1.0" ends with "-"

  @todo
  Scenario: listing shows each cached artifact's kind
    Given the service reports a cached bundle "some-registry.example/some-agent:research" of 14680064 bytes
    When the user runs image command "ls"
    Then the exit code is 0
    And the output contains "KIND"
    And the output contains "AgentSystem"

  Scenario: removing an image confirms it and reports the space reclaimed
    Given the service confirms removing "registry.example.test/some-image:1.0" reclaims 3145728 bytes
    When the user runs image command "rm some-image:1.0"
    Then the exit code is 0
    And the output contains "registry.example.test/some-image:1.0"
    And the output contains "Total reclaimed space: 3 MiB"

  Scenario: removing an image a run is using surfaces the refusal
    Given the image service refuses with "image \"registry.example.test/some-image:1.0\" in use by run #7"
    When the user runs image command "rm some-image:1.0"
    Then the exit code is 1
    And the output contains "in use by run #7"

  Scenario: pruning with --force skips the prompt and reports reclaimed space
    Given the service will prune images "registry.example.test/some-image:1.0" and "registry.example.test/other-image:2.0" reclaiming 67108864 bytes
    When the user runs image command "prune --force"
    Then the exit code is 0
    And the output contains "registry.example.test/some-image:1.0"
    And the output contains "registry.example.test/other-image:2.0"
    And the output contains "Total reclaimed space: 64 MiB"

  Scenario: pruning prompts for confirmation and proceeds on yes
    Given the service will prune images "registry.example.test/some-image:1.0" and "registry.example.test/other-image:2.0" reclaiming 67108864 bytes
    And the user answers "y" to the image prune prompt
    When the user runs image command "prune"
    Then the exit code is 0
    And the output contains "Continue? [y/N]"
    And the output contains "Total reclaimed space: 64 MiB"

  Scenario: declining the prune prompt aborts without touching the service
    Given the user answers "n" to the image prune prompt
    When the user runs image command "prune"
    Then the exit code is 0
    And the output contains "Aborted."
    And no image request reached the service

  Scenario: pruning with nothing to remove says so
    Given the service will prune no images
    When the user runs image command "prune --force"
    Then the exit code is 0
    And the output contains "No unused images."

  Scenario: an unreachable service is reported plainly
    Given the image service is unreachable
    When the user runs image command "ls"
    Then the exit code is 1
    And the output contains "no response from lns-service"
