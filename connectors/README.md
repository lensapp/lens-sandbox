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

The bundled `github`, `google`, and `openrouter` connectors authenticate the
maintainers' registered OAuth apps, whose client id (and, for `google`, client
secret) are injected from the build environment. A published artifact must not
embed those, so where a file would carry them it omits them (`github` and
`google` drop the client id; `google` also drops the secret; `openrouter`
never had either): the published connector authenticates via its token-paste
fallback or a client the consumer supplies. To publish the official app-backed
form, add the vendor's **public** client id (never the secret) before pushing.
