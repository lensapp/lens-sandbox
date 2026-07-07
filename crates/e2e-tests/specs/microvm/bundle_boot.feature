# Parked under specs/microvm/ (not the globbed features/ dir): this scenario
# boots a real AgentSystem bundle end-to-end — pull the bundle, resolve its
# sandbox + agent components, boot the sandbox baseImage as the rootfs, and run
# the agent's own command. It has no step glue yet and stays inert until a
# hermetic way to PUBLISH a bundle exists.
#
# The producer half is ready: `lns build <manifest> -t <ref> --push` assembles
# and pushes a Sandbox / Agent / Bundle artifact (see crates/lns-artifact +
# crates/lns-cli/src/build). The consumer half is wired: `lns run <bundle-ref>`
# peeks the manifest, resolves the graph, and boots the base image with the
# agent command (see peek_and_plan / bundle_launch). What's missing is a place
# to push to that the test can stand up without external services:
#
#   1. A local OCI registry in the harness (a small in-process HTTP registry
#      serving the distribution pull + chunked-push subset), and
#   2. loopback-plaintext support in lns's registry client — it defaults to
#      HTTPS, so a `127.0.0.1:<port>` registry needs the client to treat
#      loopback hosts as HTTP (as Docker/containerd do for `localhost:5000`).
#
# With both, this scenario builds+pushes the three artifacts to the local
# registry, then `lns run`s the bundle. The base image would be a small
# digest-pinned public linux image (as microvm_image_pull already pulls one).
# See CLAUDE.md "Out of scope" and the OCI-artifact PRD (lensapp/lens-sandbox#124).
@microvm
Feature: a configured agent bundle boots from an OCI registry

  Scenario: a bundle runs its agent command in the sandbox base image
    Given a local registry holding a bundle whose agent runs "echo bundle-boot-ok"
    And the bundle's sandbox base image is a small digest-pinned linux image
    When the user runs the bundle reference
    Then the exit code is 0
    And the output contains "bundle-boot-ok"

  Scenario: a --with fileset override lands on top at launch
    Given a local registry holding a bundle with a fileset mounted at "/root/.agent/config"
    When the user runs the bundle reference with --with a fileset mounting "/root/.agent/config"
    Then the exit code is 0
