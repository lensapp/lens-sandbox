Feature: publishing the image a Containerfile builds beside the document
  A document whose `spec.image` names a Containerfile publishes the image lns
  built into the artifact's own repository, so one grant covers both. The
  published document names that image by digest, keeps the path the author
  wrote in `imageSource`, and carries the Containerfile with its context as a
  layer, so a consumer never builds and an approver still reads what a build
  ran (`docs/sandbox-spec.md` §6, §7.3).

  Background:
    Given an lns.yaml whose image is "./image"
    And the file "image/Containerfile" holds:
      """
      FROM docker.io/library/node:24-bookworm
      RUN npm install -g @anthropic-ai/claude-code
      """
    And the file "image/app/main.js" holds:
      """
      console.log(1)
      """

  Scenario: a push of a path document builds the image and publishes it beside the artifact
    Given the registry accepts the push
    And the build answers with an image of 2 layers
    When the user runs artifact command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "key sha256:"
    And the output contains "built ./image/Containerfile as ghcr.io/team/hermes@sha256:"
    And the image was published into "ghcr.io/team/hermes"
    And the output contains "built and pushed ghcr.io/team/hermes:1.4.0@sha256:"

  Scenario: the published document names the index by digest and keeps the path the author wrote
    Given the registry accepts the push
    And the build answers with an image of 2 layers
    When the user runs artifact command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the published document's "spec.image" is the digest of the published index
    And the published document's "spec.imageSource" is "./image"

  Scenario: a push from a second architecture adds an entry to the index the first published
    Given the registry already holds an "amd64" image for "ghcr.io/team/hermes:1.4.0"
    And the registry accepts the push
    And the build answers with an image of 2 layers
    When the user runs artifact command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the published index holds "amd64" and "arm64"
    And the output contains "index ghcr.io/team/hermes@sha256:"

  Scenario: a push whose architecture the index already holds publishes nothing and says so
    Given the registry already holds this machine's image for "ghcr.io/team/hermes:1.4.0"
    And the registry accepts the push
    And the build answers with an image of 2 layers it did not have to build
    When the user runs artifact command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "nothing to publish: the index already holds linux/arm64"
    And no image was published

  Scenario: a push that fails after the image landed names the index it published
    Given the registry accepts 0 upload(s) then refuses
    And the build answers with an image of 2 layers
    When the user runs artifact command "push ghcr.io/team/hermes:1.4.0"
    Then the command fails with an exit code other than 0
    And the output contains "its image is already published as ghcr.io/team/hermes@sha256:"
    And the output contains "retrying is safe"

  Scenario: push --dry-run says what the index holds and what this push would add
    Given the registry already holds an "amd64" image for "ghcr.io/team/hermes:1.4.0"
    And the build answers with an image of 2 layers it did not have to build
    When the user runs artifact command "push --dry-run ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "the index holds linux/amd64 sha256:"
    And the output contains "this push would add linux/arm64 sha256:"
    And nothing is pushed

  Scenario: the Containerfile and its context ship as a layer of the same artifact
    Given the registry accepts the push
    And the build answers with an image of 2 layers
    When the user runs artifact command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the published artifact's build source layer holds "Containerfile" and "app/main.js"
    And the output contains "packed ./image/Containerfile -> sha256:"

  Scenario: a push whose image this machine already built runs nothing and says so
    Given the registry accepts the push
    And the build answers with an image of 2 layers it did not have to build
    When the user runs artifact command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "reused ./image/Containerfile as ghcr.io/team/hermes@sha256:"

  Scenario: push --dry-run prints the key and the cached digest, and builds nothing
    Given the build answers with an image of 2 layers it did not have to build
    When the user runs artifact command "push --dry-run ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "key sha256:"
    And the output contains "would publish the image ./image/Containerfile builds to, sha256:"
    And the build was asked for a plan only
    And nothing is pushed

  Scenario: push --dry-run says when a digest can only be known by building
    Given the build answers with a key and no image
    When the user runs artifact command "push --dry-run ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the output contains "key sha256:"
    And the output contains "can only be known by building"
    And nothing is pushed

  Scenario: push --rebuild ignores the cache for this push
    Given the registry accepts the push
    And the build answers with an image of 2 layers
    When the user runs artifact command "push --rebuild ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the build was asked to ignore the cache

  Scenario: a built image over the limit fails the push and names the layer that grew it
    Given the registry accepts the push
    And this machine lets a built image weigh 1024 bytes
    And the build answers with an image whose second layer is 4096 bytes
    When the user runs artifact command "push ghcr.io/team/hermes:1.4.0"
    Then the command fails with an exit code other than 0
    And the output contains "over the 1024-byte limit"
    And the output contains "RUN npm install -g @anthropic-ai/claude-code"
    And the published sandbox was not uploaded

  Scenario: a dry run of an image over the limit refuses it, as the push would
    Given this machine lets a built image weigh 1024 bytes
    And the build answers with an image whose second layer is 4096 bytes
    When the user runs artifact command "push --dry-run ghcr.io/team/hermes:1.4.0"
    Then the command fails with an exit code other than 0
    And the output contains "over the 1024-byte limit"
    And nothing is pushed

  Scenario: a push of a document that declares a mixin resolves it before the build
    Given the lns.yaml also declares a mixin
    And the service merges that document's mixin
    And the registry accepts the push
    And the build answers with an image of 2 layers
    When the user runs artifact command "push --yes ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the build was handed the resolved document and what its mixins authored

  Scenario: a push of a document that declares no mixin is built without a resolution
    Given the registry accepts the push
    And the build answers with an image of 2 layers
    When the user runs artifact command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And the builder was not asked to resolve the document

  Scenario: a pulled document re-pushed as it stands records no source it does not carry
    Given a document that was published from a Containerfile
    And the registry accepts the push
    When the user runs artifact command "push ghcr.io/team/hermes:1.5.0"
    Then the exit code is 0
    And the published document carries no "spec.imageSource"

  Scenario: a document whose image is a reference never asks for a build
    Given a valid lns.yaml in the current directory
    And the registry accepts the push
    When the user runs artifact command "push ghcr.io/team/hermes:1.4.0"
    Then the exit code is 0
    And no build was asked for
