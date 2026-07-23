# Publishable connector definitions

One file per bundled connector, in the `lns connector publish` input format (a
single connector, the same shape as an entry in
`crates/lns-policy/src/connectors.yaml`). Publish one with:

```bash
lns connector publish registry.lns.run/connectors/<id>:<version> -f connectors/<id>.yaml
```

These mirror the bundled catalog in publishable form; a test
(`crates/lns-cli/src/connector/mod.rs`) pins every file to its bundled
definition and proves it builds into a connector artifact, so the set stays
complete and in sync.

## OAuth connectors

An OAuth app's **client id is public** (OAuth spec: public clients embed it),
so it is safe in a published artifact; its **client secret is confidential**
and must never be. `build_connector` enforces this: it refuses any definition
carrying `oauth.clientSecret`.

- **`github`** carries `clientId: "${LNS_OAUTH_CLIENT_ID_GITHUB}"`. `lns
  connector publish` resolves that reference from the publisher's environment,
  so the **artifact** embeds the literal public client id while this file keeps
  the reference. Publish it with the app's public client id in scope:

  ```bash
  LNS_OAUTH_CLIENT_ID_GITHUB=<public-client-id> \
    lns connector publish registry.lns.run/connectors/github:<version> -f connectors/github.yaml
  ```

  A pulled `github` then signs in via the device flow out of the box. If the
  variable is unset, publish **fails loudly** (an unresolved `${...}` would
  break every puller's sign-in) — to publish a community, token-paste-only
  `github`, delete the `clientId` line instead.

- **`google`** deliberately omits `clientId`. Its device flow requires a
  confidential client secret, which is never published; a client id without
  that secret cannot complete the token exchange, so a published `google`
  authenticates only via its token-paste fallback until a real
  authorization-code + PKCE flow against Google lands.

- **`openrouter`** uses PKCE and needs no client id or secret at all.

**Caveat:** the catalog resolves `bundled > user > pulled` with no shadowing of
a higher tier, so a pulled connector's embedded client id only takes effect
once its id has been removed from the bundled catalog (the endpoint of the
builtins→connectors migration). Until then an official build already carries
the app-backed `github`, and a community build falls back to token paste.
