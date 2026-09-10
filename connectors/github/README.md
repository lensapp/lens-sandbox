# GitHub

Signs a sandbox in to GitHub, and puts the token in `GITHUB_TOKEN` without the
workload ever holding it. The mechanism is `kind: code`: the connector ships a
WebAssembly component that implements GitHub's device flow itself. **lns cannot
read what that component does.** It can only bound where it runs, what it
reaches, and how long it has — and it shows you those bounds before you grant
it.

## Files

- **`lns.yaml`** — the connector document. It declares what the connector serves
  (`api.github.com`, `github.com`), the one method, and the bounds lns holds the
  component to: `github.com` and nothing else, no host execution, 15 seconds per
  call, 15 minutes per connect.
- **`sign-in.wasm`** — the built component, committed so installing the
  connector needs no wasm toolchain. The connector's digest covers it, and an
  installed connector runs the bytes lns captured — so editing this file changes
  nothing until you install again, and that reinstall is what makes lns ask for
  your grant a second time.
- **`mechanism/`** — the component's source. See [Rebuild the
  component](#rebuild-the-component).

## Register a GitHub App first

No App backs this example, so you sign in through one of your own. It takes
about two minutes.

1. Go to **Settings → Developer settings → GitHub Apps → New GitHub App**.
2. Name it anything. Set the homepage URL to anything.
3. Clear **Webhook → Active**.
4. Tick **Enable Device Flow**.
5. Tick **Expire user authorization tokens**. The connector declares that it
   produces a refresh token, and lns stores a connection whole or not at all —
   an App that never expires its tokens returns none, and the connect fails
   saying so.
6. Create the App, then copy its **Client ID** (it starts with `Iv`).

## Use it

```bash
lns connector install ./connectors/github
lns connector connect github --method sign-in
```

The connect runs in rounds, because lns lends the component no listener and no
clock:

1. It asks for the Client ID. The question is the connector author's, and lns
   says so before showing it.
2. It shows a URL and a code. Open the URL, enter the code, then continue. Each
   press asks GitHub once whether you are done yet.

Then grant it to a run. The card names the bounds before you answer:

```console
$ lns connector grant github --run my-run --method sign-in
granting github to my-run would give it:
  method   Sign in with GitHub
  opens    api.github.com, github.com
  writes   nothing
  sets     GITHUB_TOKEN
  code may contact github.com
  installed at sha256:…
  lns cannot show what this code does. It can only bound where it runs, what it reaches, and how long it has.
  connection sign-in (no authority reported)
grant it? [y/N]
```

## What the component does and does not do

- It reaches `github.com` and nothing else. A call to any other host is refused
  by lns before anything leaves the machine, and written to the audit ledger.
- It runs no programs. The method declares no `exec`, so `exec.run` is refused.
- It renews itself, with no client secret. GitHub requires one to refresh a
  token *unless* that token came from the device flow, and this one did. The
  component returns a new access token, a new refresh token, and the Client ID it
  renewed with — everything the method declares it produces. A renewal that
  returned less would store nothing.
- It reports no authority. A GitHub App draws its access from the permissions the
  App was installed with, not from a scope asked for at sign-in, so the component
  asks for none and the connection lists none.
- It cannot revoke. GitHub's revocation API authenticates with the App's client
  secret, which a component you run cannot hold, so `revoke` returns an error
  rather than reporting a revocation that did not happen. `lns connector
  disconnect` drops the connection anyway and logs the error to the service — it
  does not print it — so remove the authorization yourself at
  <https://github.com/settings/applications>.

Watch what it did:

```bash
lns audit --connector github
```

## Rebuild the component

Needs `rustup target add wasm32-wasip2`.

```bash
connectors/github/mechanism/build.sh
```

`make lint` runs the same script with `--check`, which rebuilds and fails if
`sign-in.wasm` is no longer what the source produces.

If you are writing a connector of your own and you do own the App, set
`OUR_CLIENT_ID` in `src/lib.rs` to its Client ID. The component then skips the
first round and shows a code straight away. It is empty here only because no App
backs this example.
