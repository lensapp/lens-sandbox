Feature: building the image a document names
  A document's `spec.image` may name a Containerfile beside it. `lns sandbox build`
  builds that image now, fills the build cache and publishes nothing — the CI build
  stage, or a "does this even build" check before a push. It prints the key the build
  is remembered under, and a build whose key this machine already answers says so and
  runs nothing.

  Scenario: a build prints the key it is remembered under and the image it produced
    Given a document whose spec.image names a Containerfile
    And the service builds it into 3 layers
    When the user runs sandbox command "build"
    Then the exit code is 0
    And the output contains "key sha256:"
    And the output contains "built ./image/Containerfile"
    And the output contains "lns-build.local/built@sha256:"
    And the output contains "3 layers"
    And the service was asked to build "/work/lns.yaml"

  Scenario: an unchanged build is a no-op that says so
    Given a document whose spec.image names a Containerfile
    And the service answers that the key is already built
    When the user runs sandbox command "build"
    Then the exit code is 0
    And the output contains "key sha256:"
    And the output contains "nothing to build"
    And the output contains "./image/Containerfile is unchanged"

  Scenario: --rebuild ignores the cache for this build
    Given a document whose spec.image names a Containerfile
    And the service builds it into 3 layers
    When the user runs sandbox command "build --rebuild"
    Then the exit code is 0
    And the build request ignores the cache

  Scenario: -f selects the document to build
    Given a document whose spec.image names a Containerfile
    And a second document "lns.dev.yaml" whose spec.image names a Containerfile
    And the service builds it into 3 layers
    When the user runs sandbox command "build -f lns.dev.yaml"
    Then the exit code is 0
    And the service was asked to build "/work/lns.dev.yaml"

  Scenario: a document that is not there is named rather than guessed at
    Given the current directory has no lns.yaml
    When the user runs sandbox command "build"
    Then the command fails with an exit code other than 0
    And the output contains "lns.yaml"
    And the service received no request

  Scenario: a document that declares a mixin is resolved before it is built
    Given a document whose spec.image names a Containerfile and declares a mixin
    And the service resolves that document
    And the service builds it into 3 layers
    When the user runs sandbox command "build"
    Then the exit code is 0
    And the service resolved the document before it built it
    And the build carries the resolved document and what its mixins authored

  Scenario: a document that declares no mixin is built without a resolution
    Given a document whose spec.image names a Containerfile
    And the service builds it into 3 layers
    When the user runs sandbox command "build"
    Then the exit code is 0
    And the service was not asked to resolve the document

  Scenario: what the service refuses is what the user reads
    Given a document whose spec.image names a Containerfile
    And the service refuses the build with "spec.image \"alpine:3.20\" names an image to pull"
    When the user runs sandbox command "build"
    Then the command fails with an exit code other than 0
    And the output contains "names an image to pull"
