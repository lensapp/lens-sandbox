# Codex CLI in a sandbox

Runs [Codex CLI](https://developers.openai.com/codex/cli) inside a Lens Sandbox
microVM, signed in with your ChatGPT plan via
[Sign in with ChatGPT](https://learn.chatgpt.com/docs/auth?surface=app#app-sign-in-with-chatgpt).
The agent runs unrestricted **inside** the guest; the microVM and its network
policy are the boundary — nothing leaves the sandbox except the hosts you allow.

## Files

- **`lns.yaml`** — the sandbox definition: a `curlimages/curl` base that fetches
  the latest Codex CLI on first boot (Codex is a single static Rust binary —
  the musl build from its GitHub releases, no Node.js/npm needed), the network
  allowlist, and the declared port `1455` for the sign-in callback.
- **`config/`** — seed state mounted at the workload's home (`/home/sandbox`):
  - `config/.codex/config.toml` — prefers ChatGPT sign-in over an API key,
    pre-trusts `/workspace`, and runs Codex with `approval_policy = "never"` and
    `sandbox_mode = "danger-full-access"` — i.e. **Codex's own sandbox off**.
    Both are redundant here: the lns microVM is already the sandbox, so the
    agent should run freely inside it.

## How Sign in with ChatGPT works here

`codex login` (and the first-run onboarding screen) starts a small OAuth
callback server on `localhost:1455` and hands you a browser URL. Inside a
microVM that needs two things:

1. **The callback must reach the guest.** `lns.yaml` declares
   `ports: [container: 1455]`, and running your own `./lns.yaml` publishes
   declared ports automatically on host loopback — so when your host browser
   finishes the OAuth dance and redirects to `http://localhost:1455/...`, the
   request lands on Codex's server inside the guest.
2. **The guest can't open a browser.** Codex prints the sign-in URL when it
   can't launch one; open it in your host browser yourself. You sign in on
   `auth.openai.com` / `chatgpt.com` from the **host** — the guest only performs
   the token exchange (`auth.openai.com`) and the subsequent ChatGPT-plan API
   calls (`chatgpt.com`), which is why those are the allowlisted routes.

If the loopback hop doesn't suit you (e.g. you're running the sandbox on a
remote machine), `codex login --device-auth` does the same sign-in with a
one-time code and no callback port at all.

## Use it

Copy `lns.yaml` and `config/` into the project you want the agent to work on
(`.` is bound at `/workspace`, so it operates on that directory), then run:

```bash
cd your-project      # now contains lns.yaml + config/
lns run
```

The first boot downloads the latest `codex-<arch>-unknown-linux-musl` release
binary from GitHub; the microVM is ephemeral, so each cold start re-downloads
it (the `allowedRoutes` keep `github.com` and
`release-assets.githubusercontent.com` — the release-download redirect target —
open for that reason). On first run, Codex's onboarding offers **Sign in with
ChatGPT** — pick it and open the printed URL on your host.

## Publish and run from a registry

This recipe is a normal sandbox artifact: `lns push` it once and `lns run` it
by reference from any directory, exactly as walked through in the
[Claude Code example](../claude-code/README.md#publish-and-run-from-a-registry).
One codex-specific caveat: a *pulled* sandbox's declared ports are disclosure,
not a grant — a bare `lns run ghcr.io/you/codex:0.1.0` publishes nothing, so
pass `-P` (or `-p 1455:1455`) when you need to sign in, or use
`codex login --device-auth` and skip the port entirely.

## Notes

- Sign in with ChatGPT stores its tokens at `~/.codex/auth.json` **inside the
  guest**, and everything except `/workspace` is ephemeral — a cold start asks
  you to sign in again. To keep the login across runs, attach a named volume
  over the Codex home (`-v codex-home:/home/sandbox/.codex`); note it shadows
  the seeded `config.toml`, so Codex re-asks its onboarding questions once and
  then persists everything, credentials included, in the volume.
- Prefer keeping credentials out of the guest entirely? Bind the `openai`
  integration instead (declare `integrations: [openai]`): the workload sees an
  `OPENAI_API_KEY`-shaped placeholder and the real key is injected at the
  network boundary. That's metered API-key auth, not your ChatGPT plan.
- The network allowlist is deliberately tight; anything else the agent reaches
  for pauses on an approval card (`defaultVerdict: ask`). If you bake Codex
  into a custom image and `lns push` it, you can drop the two GitHub routes
  and the first-boot download entirely.
- The base image matters more than it looks: the guest's egress goes through
  the sandbox's filtering proxy, and busybox `wget` can't speak HTTPS through
  it — `curl` can, which is why the base is `curlimages/curl` rather than bare
  `alpine`.
