Feature: publishing a connector to a registry
  `lns connector publish <ref> -f <file>` builds a connector definition
  into a config-only connector artifact and uploads it to an OCI
  registry, reusing the `lns login` credential. Publishing is inert on
  its own — an artifact only becomes reachable once its id is added to
  the discovery index — so this is the safe first half of distributing
  connectors from a registry. A definition carrying an oauth client
  secret is refused, because a registry artifact would embed it in the
  clear.

  These scenarios use a synthetic connector definition, so nothing here
  pins a shipped service.

  Scenario: Publishing uploads the definition as a connector artifact
    Given a connector definition file "some-provider.yaml" declares a credential connector
    When the user runs connector command "publish registry.lns.run/connectors/some-provider:0.1.0 -f some-provider.yaml"
    Then the exit code is 0
    And the output contains "built and pushed registry.lns.run/connectors/some-provider:0.1.0@sha256:"

  Scenario: A dry-run builds and validates without uploading
    Given a connector definition file "some-provider.yaml" declares a credential connector
    When the user runs connector command "publish registry.lns.run/connectors/some-provider:0.1.0 -f some-provider.yaml --dry-run"
    Then the exit code is 0
    And the output contains "would push"
    And the output contains "nothing uploaded"

  Scenario: Publishing refuses a connector carrying an oauth client secret
    Given a connector definition file "leaky.yaml" declares an oauth connector carrying a client secret
    When the user runs connector command "publish registry.lns.run/connectors/leaky:0.1.0 -f leaky.yaml"
    Then the output contains "clientSecret"
