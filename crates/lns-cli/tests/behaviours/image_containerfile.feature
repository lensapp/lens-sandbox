Feature: an image built from a Containerfile beside the document
  `spec.image` names either an OCI reference or a Containerfile beside the
  document. The path form is held to the same boundary a fileset path is —
  the artifact ships what it names — and the document says which file a build
  would use before anything is built.

  Scenario: a directory holding a Containerfile is an image the document may name
    Given an lns.yaml whose image is "./image"
    And the directory "image" holds a "Containerfile"
    When the user runs artifact command "validate"
    Then the exit code is 0
    And the output contains "valid"

  Scenario: a directory holding neither name is refused, so the typo is found before the build
    Given an lns.yaml whose image is "./image"
    And the directory "image" holds a "README.md"
    When the user runs artifact command "validate"
    Then the exit code is 1
    And the output contains "spec.image"
    And the output contains "Containerfile"

  Scenario: an image path that leaves the document's directory is refused
    Given an lns.yaml whose image is "../elsewhere"
    When the user runs artifact command "validate"
    Then the exit code is 1
    And the output contains "spec.image"
    And the output contains "../elsewhere"

  Scenario: a home-anchored image path is refused
    Given an lns.yaml whose image is "~/image"
    When the user runs artifact command "validate"
    Then the exit code is 1
    And the output contains "home-anchored"

  Scenario: inspect discloses the Containerfile a build would use
    Given an lns.yaml whose image is "./image"
    And the directory "image" holds a "Dockerfile"
    When the user runs artifact command "inspect"
    Then the exit code is 0
    And the output contains "built from ./image/Dockerfile"

  Scenario: inspect prints the Containerfile itself, so an approver reads what a build would run
    Given an lns.yaml whose image is "./image"
    And the file "image/Dockerfile" holds:
      """
      FROM docker.io/library/node:24-bookworm
      RUN npm ci
      """
    And the file "image/app/main.js" holds:
      """
      console.log("hermes");
      """
    When the user runs artifact command "inspect"
    Then the exit code is 0
    And the output contains "built from ./image/Dockerfile (2 lines, context 2 files)"
    And the output contains "RUN npm ci"

  Scenario: inspect lists the context a build would send, with each file's size
    Given an lns.yaml whose image is "./image"
    And the file "image/Dockerfile" holds:
      """
      FROM docker.io/library/node:24-bookworm
      """
    And the file "image/app/main.js" holds:
      """
      console.log("hermes");
      """
    When the user runs artifact command "inspect"
    Then the exit code is 0
    And the output contains "context:      Dockerfile (40 B)"
    And the output contains "context:      app/main.js (23 B)"

  Scenario: a symlink in the context is listed as one a build does not send
    Given an lns.yaml whose image is "./image"
    And the file "image/Dockerfile" holds:
      """
      FROM docker.io/library/node:24-bookworm
      """
    And the file "image/node_modules/.bin/tsc" is a symlink
    When the user runs artifact command "inspect"
    Then the exit code is 0
    And the output contains "built from ./image/Dockerfile (1 lines, context 1 files)"
    And the output contains "context:      node_modules/.bin/tsc (symlink, not sent)"

  Scenario: a directory holding both builds the Containerfile, as Podman does
    Given an lns.yaml whose image is "./image"
    And the directory "image" holds a "Containerfile"
    And the directory "image" holds a "Dockerfile"
    When the user runs artifact command "inspect"
    Then the exit code is 0
    And the output contains "built from ./image/Containerfile"

  Scenario: a path naming a file is the Containerfile whatever it is called
    Given an lns.yaml whose image is "./image/base.containerfile"
    And the directory "image" holds a "base.containerfile"
    When the user runs artifact command "validate"
    Then the exit code is 0
    And the output contains "valid"

  Scenario: a run of such a document says building is not yet supported
    Given an lns.yaml whose image is "./image"
    And the directory "image" holds a "Containerfile"
    When the user runs "lns run"
    Then the command fails with an exit code other than 0
    And the output contains "not yet supported"
    And the output contains "./image"

  Scenario: a push of such a document refuses before it publishes anything
    Given an lns.yaml whose image is "./image"
    And the directory "image" holds a "Containerfile"
    And the registry accepts the push
    When the user runs artifact command "push ghcr.io/team/hermes:1.4.0"
    Then the command fails with an exit code other than 0
    And the output contains "not yet supported"
    And the published sandbox was not uploaded

  Scenario: an image path naming nothing is refused where a fileset's typo is
    Given an lns.yaml whose image is "./image"
    When the user runs artifact command "validate"
    Then the exit code is 1
    And the output contains "names no file or directory"
