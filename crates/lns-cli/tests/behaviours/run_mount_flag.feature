Feature: Mounting an application-layer artifact with --mount

  `lns run <ref> --mount <fileset-ref>` attaches an application-layer artifact
  (a fileset, model, tool, or knowledge) into the guest at the path the artifact
  declares, so a non-compliant agent can be fed config it reads from a fixed path.

  Scenario: a fileset is mounted at its declared path
    Given a fileset artifact "localhost:5000/org/acme/filesets/hermes-config:v1" mounting at "/opt/data"
    When the developer runs an image with mount "localhost:5000/org/acme/filesets/hermes-config:v1"
    Then an artifact mount targets "/opt/data" from "localhost:5000/org/acme/filesets/hermes-config:v1"

  Scenario: an explicit path overrides the declared mount
    Given a fileset artifact "localhost:5000/org/acme/filesets/hermes-config:v1" mounting at "/opt/data"
    When the developer runs an image with mount "localhost:5000/org/acme/filesets/hermes-config:v1:/srv/cfg"
    Then an artifact mount targets "/srv/cfg" from "localhost:5000/org/acme/filesets/hermes-config:v1"
