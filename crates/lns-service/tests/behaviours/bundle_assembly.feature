Feature: a bundle assembles into a composed workload
  A bundle (AgentSystem) composes a sandbox's base image with one or more
  filesets and the agent's invocation. Filesets are a last-wins overlay:
  the base image sits at the bottom, filesets stack in bundle-declaration
  order, and a path present in a later layer overrides an earlier one — so
  the layer that owns each mount target is deterministic regardless of pull
  order. The agent contributes only the invocation (command and env); the
  base image it runs on comes from the sandbox.

  Scenario: The sandbox base image is the bottom layer
    Given a bundle whose sandbox base image is "registry.example.test/base:1"
    And the bundle declares no filesets
    When the bundle is assembled
    Then the assembled workload runs from base image "registry.example.test/base:1"

  Scenario: A fileset is overlaid onto the base image at its mount path
    Given a bundle whose sandbox base image is "registry.example.test/base:1"
    And the bundle declares a fileset "settings" mounting "/root/.some-agent/settings.json"
    When the bundle is assembled
    Then "/root/.some-agent/settings.json" in the assembled workload comes from fileset "settings"

  Scenario: A fileset overrides a path the base image ships
    Given a bundle whose sandbox base image ships "/root/.some-agent/settings.json"
    And the bundle declares a fileset "override" mounting "/root/.some-agent/settings.json"
    When the bundle is assembled
    Then "/root/.some-agent/settings.json" in the assembled workload comes from fileset "override"

  Scenario: A later fileset wins a same-target collision with an earlier one
    Given a bundle whose sandbox base image is "registry.example.test/base:1"
    And the bundle declares a fileset "first" mounting "/root/.some-agent/settings.json"
    And the bundle declares a fileset "second" mounting "/root/.some-agent/settings.json"
    When the bundle is assembled
    Then "/root/.some-agent/settings.json" in the assembled workload comes from fileset "second"

  Scenario: The assembled workload runs the agent's command and env
    Given a bundle whose agent invocation runs command "agent --serve" with env "MODE=research"
    When the bundle is assembled
    Then the assembled workload launches command "agent --serve"
    And the assembled workload's environment carries "MODE=research"
