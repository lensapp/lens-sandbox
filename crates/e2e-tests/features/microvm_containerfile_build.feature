@microvm
Feature: a run whose spec.image is a Containerfile builds it and boots from the result
  Slice 3 of lensapp/lens-sandbox#393, the executor. `spec.image` is a path beside the
  document, so the service runs the Containerfile instruction by instruction before
  anything boots: `FROM` pulls through the ordinary image path, each `RUN` runs in a
  build guest through the guest exec every workload uses and what it wrote becomes one
  layer, `COPY` becomes one layer of the context's files, and `ENV`, `USER`, `WORKDIR`
  and `CMD` write the image config. The finished image goes into the local layer store
  and the run boots it.

  Every effect is asserted from inside the booted guest, which is the only place that
  can tell a built image from a built manifest: the file the `RUN` created, the file the
  `COPY` brought, the variable the `ENV` set, the directory the `WORKDIR` made the
  workload's own, and the command the `CMD` gave a run that names none.

  Like every @microvm scenario this boots real guests and reaches a real registry for
  the base image, so it runs only via `make e2e-microvm`, never in CI's PR gate. It
  boots one guest per filesystem instruction, then one for the run itself.

  Scenario: every instruction of a Containerfile reaches the image the run boots
    Given a clean lns cache home
    And the LNS service is running in that home
    And the build context holds "app/index.js" containing "copied-by-lns"
    And the project builds its image from this Containerfile
      """
      FROM {base}
      ENV AGENT_MODE=research
      RUN /bin/sh -c 'echo built-by-lns > /built-marker'
      COPY app /srv/app
      USER root
      WORKDIR /srv
      CMD ["/bin/sh", "-c", "echo cmd-from-the-containerfile"]
      """
    When the user runs a microVM command "/bin/sh -c 'cat /built-marker; cat /srv/app/index.js; echo mode=$AGENT_MODE; echo wd=$(/.lens/guest-tools/bin/busybox pwd)'"
    Then the exit code is 0
    And the output contains "built-by-lns"
    And the output contains "copied-by-lns"
    And the output contains "mode=research"
    And the output contains "wd=/srv"
    And the run reports the image it built from "./image/Dockerfile"
    When the user runs the sandbox definition
    Then the exit code is 0
    And the output contains "cmd-from-the-containerfile"
    And the output does not contain "Building"
    And the run reports the image it built from "./image/Dockerfile"
