# GitHub

Signs a sandbox in to GitHub, and puts the token in `GITHUB_TOKEN` without the
workload ever holding it. The mechanism is `kind: code`: the connector ships a
WebAssembly component that implements GitHub's device flow itself. **lns cannot
read what that component does.** It can only bound where it runs, what it
reaches, and how long it has — and it shows you those bounds before you grant
it.

## Two methods, because GitHub has two kinds of app

You register one or the other, and you pick the matching method. They differ in
what GitHub hands back, which is what a method has to declare up front.

| | `sign-in` | `oauth-sign-in` |
|---|---|---|
| Register a | GitHub App | OAuth App |
| Token | expires in 8 hours | never expires |
| Renews itself | yes, and rotates both tokens | no, and nothing has to |
| Access comes from | the App's installed permissions | the scopes you ask for |
| A new one's Client ID | `Iv23li…` | `Ov23li…` |

Use `sign-in` if you want the token to rotate. Use `oauth-sign-in` if you want to
choose the scopes, or if you already have an OAuth App.

They cannot be one method: a method declares its outputs before it runs, lns
stores a connection whole or not at all, and an OAuth App never returns the
refresh token the `sign-in` method promises.

## Files

- **`lns.yaml`** — the connector document. It declares what the connector serves
  (`api.github.com`, `github.com`), both methods, and the bounds lns holds each
  component to: `github.com` and nothing else, no host execution, 15 seconds per
  call, 15 minutes per connect.
- **`sign-in.wasm`**, **`oauth-sign-in.wasm`** — the built components, one per
  method, committed so installing the connector needs no wasm toolchain. The
  connector's digest covers them, and an installed connector runs the bytes lns
  captured — so editing one changes nothing until you install again, and that
  reinstall is what makes lns ask for your grant a second time.
- **`mechanism/`** — one crate, built twice. See [Rebuild the
  components](#rebuild-the-components).

## Register an app first

No app backs this example, so you sign in through one of your own. Either takes
about two minutes.

**A GitHub App, for `sign-in`:**

1. Go to **Settings → Developer settings → GitHub Apps → New GitHub App**.
2. Name it anything. Set the homepage URL to anything.
3. Clear **Webhook → Active**.
4. Tick **Enable Device Flow**.
5. Create the App, then copy its **Client ID**.
6. Leave user-token expiration on. It is on by default; if someone opted this App
   out, turn it back on under **Edit → Optional Features → User-to-server token
   expiration → Opt-in**. Without it GitHub returns no refresh token, and the
   connect fails saying so.

**An OAuth App, for `oauth-sign-in`:**

1. Go to **Settings → Developer settings → OAuth Apps → New OAuth App**.
2. Name it anything. Set the homepage and callback URLs to anything.
3. Create it, then on its page tick **Enable Device Flow**.
4. Copy its **Client ID**.

## Use it

```bash
lns connector install ./connectors/github
lns connector connect github --method sign-in        # or --method oauth-sign-in
```

`--method` is required, because this connector offers two.

The connect runs in rounds, because lns lends the component no listener and no
clock:

1. It asks for the Client ID — and, for `oauth-sign-in`, for the scopes the token
   should carry. The questions are the connector author's, and lns says so before
   showing them.
2. It shows a URL and a code. Open the URL, enter the code, then continue. Each
   press asks GitHub once whether you are done yet.

Scopes are yours to choose, and only `oauth-sign-in` asks: `public_repo
read:user` is a reasonable start, `repo` reaches private repositories, and an
empty answer asks for nothing beyond public read-only data. A GitHub App ignores
scopes entirely, so `sign-in` never asks.

Then grant it to a run. The card names the bounds before you answer:

```console
$ lns connector grant github --run my-run --method sign-in
granting github to my-run would give it:
  method   Sign in with a GitHub App
  opens    api.github.com, github.com
  writes   nothing
  sets     GITHUB_TOKEN
  code may contact github.com
  installed at sha256:…
  lns cannot show what this code does. It can only bound where it runs, what it reaches, and how long it has.
  connection sign-in (no authority reported)
grant it? [y/N]
```

## What the components do and do not do

Both:

- Reach `github.com` and nothing else. A call to any other host is refused by lns
  before anything leaves the machine, and written to the audit ledger.
- Run no programs. Neither method declares `exec`, so `exec.run` is refused.
- Cannot revoke. GitHub's revocation API authenticates with the app's client
  secret, which a component you run cannot hold, so `revoke` returns an error
  rather than reporting a revocation that did not happen. `lns connector
  disconnect` drops the connection anyway and logs the error to the service — it
  does not print it — so remove the authorization yourself at
  <https://github.com/settings/applications>.

`sign-in` also:

- Renews itself, with no client secret. GitHub requires one to refresh a token
  *unless* that token came from the device flow, and this one did. The component
  returns a new access token, a new refresh token, and the Client ID it renewed
  with — everything the method declares it produces. A renewal that returned less
  would store nothing.
- Reports no authority, because a GitHub App's access is the App's installed
  permissions rather than anything asked for at sign-in.

`oauth-sign-in` also:

- Reports the scopes GitHub actually granted, which is not always what was asked
  for.
- Refuses a token that expires. This method keeps no refresh token, so every
  renewal of one would fail and you would redo the device flow every few hours.
  It sends you to `sign-in` with a GitHub App instead.

Watch what they did:

```bash
lns audit --connector github
```

## Rebuild the components

Needs `rustup target add wasm32-wasip2`.

```bash
connectors/github/mechanism/build.sh
```

One crate, two cargo features, two committed `.wasm` files — a component is told
nothing about which method invoked it, so each method gets its own build. `make
lint` runs the same script with `--check`, which rebuilds both and fails if
either is no longer what the source produces.

If you are writing a connector of your own and you do own the app, set
`OUR_CLIENT_ID` in `src/lib.rs` to its Client ID. The components then stop asking
for one. It is empty here only because no app backs this example.
