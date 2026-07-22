# Codex CLI in a sandbox (OpenAI API key)

Runs [Codex CLI](https://developers.openai.com/codex/cli) inside a Lens Sandbox
microVM with usage-based API-key billing through the bundled `openai`
integration. The real key never enters the guest: the workload holds an
`OPENAI_API_KEY`-shaped **placeholder**, and the boundary swaps in your real
key only on requests to `api.openai.com`. For ChatGPT-plan billing instead,
see the sibling [`codex-chatgpt-subscription`](../codex-chatgpt-subscription/)
example.

Compared to that sibling, this is the stateless variant: no sign-in flow, so
no callback port to publish, and no token state to persist, so no named
volume — every boot is a fresh microVM.

## Files

- **`lns.yaml`** — the sandbox definition: a `curlimages/curl` base that
  fetches the latest Codex CLI on first boot (a single static Rust binary from
  its GitHub releases), the network allowlist, and the `openai` integration.
- **`config/`** — seed state mounted at the workload home (`/home/curl_user`,
  the image's user):
  - `config/.codex/config.toml` — prefers API-key auth, pre-trusts
    `/workspace`, and runs Codex with `approval_policy = "never"` and
    `sandbox_mode = "danger-full-access"` — i.e. **Codex's own sandbox off**.
    Both are redundant here: the lns microVM is already the sandbox, so the
    agent should run freely inside it.

At boot, the command pipes the placeholder into `codex login --with-api-key`
(an offline, in-guest step — Codex just records the key shape) and starts the
agent. Codex then sends the placeholder as a bearer token to `api.openai.com`,
where the boundary injects your real key.

## Use it

Copy `lns.yaml` and `config/` into the project you want the agent to work on
(`.` is bound at `/workspace`), connect the integration once, and run:

```bash
cd your-project      # now contains lns.yaml + config/
lns integration connect openai   # bind your real API key (per-machine, ~/.lns-credentials.json)
lns run
```

Declaring `integrations: [openai]` in `lns.yaml` is disclosure, not a grant —
the connect step (or accepting the approval card on first use) is what arms it
and records it in this directory's `lns-policy.yaml`. Your key lives in
`~/.lns-credentials.json` on your machine, never in the guest, the project, or
a published artifact.

## Publish and run from a registry

This recipe is a normal sandbox artifact: `lns push` it once and `lns run` it
by reference from any directory, exactly as walked through in the
[Claude Code example](../claude-code/README.md#publish-and-run-from-a-registry).
A consumer connects their own `openai` key — the artifact carries no
credential, and a declared integration is never armed behind their back.

## Notes

- Each cold boot re-downloads the ~100MB Codex binary (the `github.com` and
  `release-assets.githubusercontent.com` routes exist for that). If that
  grates, bake Codex into a custom image and drop those routes — or accept a
  `codex-home` named volume like the subscription sibling, trading
  statelessness for a warm start.
- The network allowlist is deliberately tight; anything else the agent
  reaches for pauses on an approval card (`defaultVerdict: ask`).
- API-key auth is usage-based billing against your OpenAI platform account —
  spend caps live there, and the audit chain records every request the
  boundary rewrote.
