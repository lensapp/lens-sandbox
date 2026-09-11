# Native OAuth connector templates

These templates require a public client registration. Replace
`REPLACE_WITH_REGISTERED_PUBLIC_CLIENT_ID` in a local copy with an ID you or your
connector publisher registered. The ID is public configuration; it is not a
credential and does not establish trust in a connector publisher. Review the
connector before installing and its disclosure before granting access.

Standard OAuth runs natively in LNS, without a Wasm component. `kind: code`
remains available for authentication requiring custom behavior. These built-ins
do not support confidential clients, client secrets, dynamic registration,
provider discovery, or a hosted broker.

## GitHub device authorization

1. Register your own GitHub OAuth app and enable **Device Flow** in its settings.
   Follow GitHub's [device-flow setup and protocol instructions](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow).
2. Copy [github-device/lns.yaml](github-device/lns.yaml) to a local directory.
   Set `clientId` to that app's public client ID. No client secret is needed.
   The template requests `read:user`, a read-only profile scope.
3. Install and connect:

   ```sh
   lns connector install ./github-device/lns.yaml
   lns connector connect github-native --method device --as personal
   ```

LNS shows the verification destination and user code and opens the browser.
Check the displayed code, sign in, and consent there. The service polls
automatically; do not press Continue. Ctrl-C cancels the terminal operation.
GitHub may return no refresh token or expiry; LNS keeps those facts as returned
and does not schedule an impossible renewal.

## Linear native authorization code

1. Register your own OAuth application in Linear, following its
   [OAuth documentation](https://linear.app/developers/oauth-2-0-authentication).
   Configure a public-client-compatible app using PKCE and register exactly
   `http://127.0.0.1:53682/callback` as its callback.
2. Copy [linear-native/lns.yaml](linear-native/lns.yaml) locally and replace
   `clientId`. Keep the callback configuration identical to the registration.
   Linear documents secretless PKCE code exchange and refresh. If your
   registration requires a secret, this template cannot use it: resolve the
   provider registration rather than adding a secret to the connector.
3. Install and connect:

   ```sh
   lns connector install ./linear-native/lns.yaml
   lns connector connect linear-native --method browser --as personal
   ```

LNS binds only `127.0.0.1`, opens the system browser with fresh PKCE S256 and state,
and waits automatically for the callback. An occupied port fails with an
explanation; LNS does not select an unregistered replacement. Providers allowing
variable loopback ports can use an omitted `redirect.port`; register their
supported loopback callback form first. The actual callback URI appears in LNS.

The Rust approval card supports the same sign-in, including Open browser,
Copy code for device flow, and Cancel. Completion advances automatically. The
Approvals list can grant an existing connection; start sign-in on the card or
in the CLI first. Noninteractive `connect` fails without launching a browser.

## Grant and verify direct API access

Connecting an account grants no run access. Separately approve the connector for
the intended sandbox, using its approval card or, for example:

```sh
lns connector grant github-native --run my-sandbox --method device --connection personal
lns connector grant linear-native --run my-sandbox --method browser --connection personal
```

The workload receives a placeholder in `GITHUB_TOKEN` or `LINEAR_TOKEN`. LNS
substitutes the access token only at the declared API boundary. Renewal state
stays in the host credential store and cannot be selected as a workload output.

For acceptance testing, run a read-only request inside that granted sandbox:
GitHub `GET https://api.github.com/user` with `Authorization: Bearer $GITHUB_TOKEN`
and `Accept: application/vnd.github+json`; Linear
`POST https://api.linear.app/graphql` with `Authorization: Bearer $LINEAR_TOKEN`,
JSON content type, and body `{"query":"{ viewer { id } }"}`. Check the HTTP result
and requested identity field without printing credentials. Use an image with
an HTTP client installed. An MCP server accepting a token does not establish
that either direct API accepts it.

These are setup templates, not registrations or a claim of live provider
success. A demonstration also requires user browser consent and direct API
verification. Use a separate `LNS_HOME`, `LNS_SOCKET_PATH`, and service binary so
the demonstration leaves your normal service alone. Refresh is live-proven only
if a renewable account is actually exercised; mocked refresh tests do not prove
provider behavior.

LNS retains rotated refresh tokens, preserves scopes omitted on refresh, and
requires renewed grant consent when authority changes. It disarms expired or
invalid credentials and retries temporary renewal failures with bounded backoff.
Changing a connector's client identity or token endpoint requires reconnecting.
`lns connector disconnect` removes local credentials and cancels pending work;
it does **not** revoke provider-side authorization. Revoke that separately in the
provider's account settings when needed.
