@microvm
Feature: a document built from a Containerfile publishes its image and pulls back whole
  Slice 5 of lensapp/lens-sandbox#393. `lns push` of a document whose `spec.image`
  is a path builds the Containerfile, publishes the built image into the artifact's
  own repository — so one grant covers both — and uploads a document that names that
  image by digest, keeps the path in `imageSource`, and carries the Containerfile
  with its context as a layer.

  What that is worth is only visible on a second machine: a home that has never
  built anything pulls the artifact, inspects it and reads the instructions off it
  without booting anything, then runs it and finds the files the build wrote — with
  no `pre-start` script anywhere in the document.

  Like every @microvm scenario this boots real guests and reaches a real registry
  for the base image, so it runs only via `make e2e-microvm`, never in CI's PR gate.

  Scenario: a pushed Containerfile image pulls back and runs on a machine that never built it
    Given a clean lns cache home
    And a local registry
    And the LNS service is running in that home
    And the build context holds "app/index.js" containing "copied-by-lns"
    And the project builds its image from this Containerfile
      """
      FROM {base}
      RUN /bin/sh -c 'echo built-by-lns > /built-marker'
      COPY app /srv/app
      """
    When the user pushes the sandbox definition to the local registry
    Then the exit code is 0
    And the output contains "built ./image/Dockerfile as"
    And the output contains "packed ./image/Dockerfile -> sha256:"
    And the output contains "built and pushed"
    And the image was published into the artifact's own repository

    When the user pulls the pushed sandbox onto a machine that has never built it
    Then the exit code is 0
    And the output contains "pulled"

    When the user inspects the pushed sandbox
    Then the exit code is 0
    And the published document names its image by digest in the artifact's own repository
    And the output contains "imageSource: built from ./image/Dockerfile"
    And the output contains "context: app/index.js"
    And the output contains "RUN /bin/sh -c 'echo built-by-lns > /built-marker'"
    And the inspect booted no guest

    When the user runs the pushed sandbox with "/bin/sh -c 'cat /built-marker; cat /srv/app/index.js'"
    Then the exit code is 0
    And the output contains "built-by-lns"
    And the output contains "copied-by-lns"
    And the output does not contain "Building"
    And the document that ran declared no pre-start script
