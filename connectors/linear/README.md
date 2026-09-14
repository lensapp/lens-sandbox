# Linear MCP authentication probe

This connector tests whether Linear's MCP browser sign-in produces a credential
that also works with the direct GraphQL API. It requests read-only access and
saves a connection only after both interfaces accept the same token.

Live browser authorization completed, but the direct API returned **HTTP 401**
for the resulting token (confirmed in a diagnostic retry). Treat this as a compatibility probe, not a supported direct API connector.
A successful MCP sign-in alone does not establish permission to use another API.

## Try it

Use the service and CLI built from this branch:

```sh
lns connector install ./connectors/linear
lns connector connect linear --method sign-in
```

Continue at the first prompt to open Linear in your browser. Approve read access,
wait for the browser to say **Authorization received**, then continue in the CLI.
If Linear sends a new account through onboarding, finish onboarding and restart
`lns connector connect` to return to authorization.

For an isolated CLI-only demo from the repository root, keep its service and
connector data separate from your usual LNS instance:

```sh
make dev
export LNS_HOME="$(mktemp -d /tmp/lns-linear.XXXXXX)"
export LNS_SOCKET_PATH="$LNS_HOME/service.sock"
export LNS_SERVICE_BIN="$PWD/target/debug/lns-service"
LNS_HEADLESS=1 target/debug/lns service start
target/debug/lns connector install ./connectors/linear
target/debug/lns connector connect linear --method sign-in
target/debug/lns connector list
target/debug/lns service stop
unset LNS_HOME LNS_SOCKET_PATH LNS_SERVICE_BIN
```

The existing approval window can drive the same sign-in rounds when a run
requests a destination this connector serves. No native macOS app changes are
included.

The component discovers Linear's MCP authorization endpoints, dynamically
registers an OAuth public client, and uses S256 PKCE with an LNS-owned loopback
callback. It then calls `viewer { id }` on `api.linear.app/graphql` and initializes
an MCP session at `mcp.linear.app/mcp`. These checks do not change Linear data.
The CLI reports a failure without saving a connection if either check fails.

When both checks succeed, the connector returns the access token, refresh token,
client ID, granted scopes, and expiry for LNS's existing renewal scheduler. It
uses the MCP token endpoint for refresh. Live renewal still needs verification
with a successfully connected account.

A granted run receives a placeholder in `LINEAR_ACCESS_TOKEN`; LNS substitutes
the credential in Bearer headers only for the two declared destinations. CLI
programs can use that placeholder in ordinary HTTP calls. Real credentials stay
outside the workload.

## Limits

- This example pins Linear's expected OAuth endpoints; it is not a generic MCP
  discovery client or a marketplace importer.
- LNS limits browser navigation to HTTPS on declared hosts. The callback accepts
  one authorization code with the expected state, expires within the connect
  session, and is unavailable during refresh or revocation.
- The browser may follow provider redirects during sign-in.
- Disconnecting removes the local connection. This example does not revoke the
  provider grant; remove the LNS authorization in Linear's application settings.
- A published OCI connector can carry the same manifest and component using the
  PR's existing packaging. No registry entry has been published for this probe.

## Rebuild

```sh
rustup target add wasm32-wasip2
connectors/linear/mechanism/build.sh
connectors/linear/mechanism/build.sh --check
```

The committed component lets users install without a Wasm toolchain. Reinstall
it after rebuilding; installed connectors use captured bytes.

See [Linear's MCP documentation](https://linear.app/docs/mcp) and
[OAuth documentation](https://linear.app/developers/oauth-2-0-authentication).
Linear documents using an existing API OAuth token with MCP; that does not
promise the reverse direction works.
