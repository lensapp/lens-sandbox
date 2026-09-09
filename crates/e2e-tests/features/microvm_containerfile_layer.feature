@microvm
Feature: one guest's writes become one OCI layer a second guest boots from
  Slice 1 of lensapp/lens-sandbox#393, the layer round trip, with no Containerfile
  parser and one hard-coded command. The service runs with the slice-1 capture hook
  set, so when the run's command exits and the guest powers off, the host reads the
  run's `upper.img`, writes what it finds as one uncompressed OCI layer with
  whiteouts, assembles a config and manifest over the base image, and imports the
  result where the ordinary boot path finds it. The second run boots that built
  image and proves both halves of the change: the file the first guest created is
  there, and the file the base image shipped and the first guest deleted is not.

  Like every @microvm scenario this boots a real guest and reaches a real registry
  for the base image, so it runs only via `make e2e-microvm`, never in CI's PR gate.

  The command runs as root: the workload identity a run drops to owns nothing in
  the base image's rootfs, so an unprivileged one can neither create a file at `/`
  nor delete one from `/etc`, and this scenario is about capturing both.

  Scenario: the second guest sees the created file and not the deleted one
    Given a clean lns cache home
    And the LNS service is running in that home with the layer-capture hook
    When the user runs a microVM command "/bin/sh -c 'echo built-by-lns > /spike-created; rm -f /etc/alpine-release'" as user "root"
    Then the exit code is 0
    And one OCI layer was captured from that run
    When the user runs a microVM command "/bin/sh -c 'cat /spike-created; test -e /etc/alpine-release || echo release-file-gone'" over the built image
    Then the exit code is 0
    And the output contains "built-by-lns"
    And the output contains "release-file-gone"
