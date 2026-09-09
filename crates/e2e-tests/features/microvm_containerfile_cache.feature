@microvm
Feature: a build is remembered by its key, and only what changed is built again
  Slice 4 of lensapp/lens-sandbox#393, the build cache and `lns sandbox build`. The
  verb builds what `spec.image` names, fills the cache and publishes nothing. The image
  is keyed by the `FROM` digest, the Containerfile's text, the context's content hash
  and the architecture, so a second build of an untouched document runs nothing and
  says so. One instruction is keyed by the image it stands on and by what it resolved
  to, so an edited context file rebuilds the `COPY` that carries it and everything
  after it, and nothing before it.

  What ran is read two ways: from what the verb printed, and from the instructions the
  service booted a guest for, which it names one line each in its own log. A `RUN` no
  key answered is a boot; a `RUN` a key answers is not.

  Like every @microvm scenario this boots real guests and reaches a real registry for
  the base image, so it runs only via `make e2e-microvm`, never in CI's PR gate.

  Scenario: an unchanged build runs nothing, and an edited context file rebuilds from the copy that carries it
    Given a clean lns cache home
    And the LNS service is running in that home
    And the build context holds "app/index.js" containing "copied-by-lns"
    And the project builds its image from this Containerfile
      """
      FROM {base}
      RUN /bin/sh -c 'echo built-by-lns > /built-marker'
      COPY app /srv/app
      RUN /bin/sh -c 'cat /srv/app/index.js > /srv/derived'
      """
    When the user builds the sandbox definition
    Then the exit code is 0
    And the build reports the key it is remembered under
    And the build reports the image it produced from "./image/Dockerfile"
    And the service booted a guest for 2 instructions of that build
    When the user builds the sandbox definition
    Then the exit code is 0
    And the output contains "nothing to build"
    And the build reports the same key as the build before it
    And the service booted a guest for 0 instructions of that build
    When the build context file "app/index.js" is changed to "edited-by-hand"
    And the user builds the sandbox definition
    Then the exit code is 0
    And the build reports a different key from the build before it
    And the service booted a guest for 1 instruction of that build
    When the user runs a microVM command "/bin/sh -c 'cat /built-marker; cat /srv/derived'"
    Then the exit code is 0
    And the output contains "built-by-lns"
    And the output contains "edited-by-hand"
    And the output does not contain "Building"
