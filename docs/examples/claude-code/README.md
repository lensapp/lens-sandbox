# Claude Code in a sandbox

Runs [Claude Code](https://docs.claude.com/en/docs/claude-code) inside a Lens
Sandbox microVM. The agent runs unrestricted **inside** the guest; the microVM
and its network policy are the boundary — nothing leaves the sandbox except the
hosts you allow.

## Files

- **`lns.yaml`** — the sandbox definition: a `node:lts-alpine` base that installs
  Claude Code on first boot, the network allowlist, and the
  `claude-code-subscription` integration.
- **`config/`** — seed state mounted at the workload's home (`/home/sandbox`):
  - `config/.claude.json` — skips onboarding and pre-accepts the `/workspace`
    trust dialog.
  - `config/.claude/settings.json` — runs Claude in `bypassPermissions` and turns
    **off Claude's own sandbox**. Both are redundant here: the lns microVM is
    already the sandbox, so the agent should run freely inside it.

## Use it

Copy `lns.yaml` and `config/` into the project you want the agent to work on
(`.` is bound at `/workspace`, so it operates on that directory), then run:

```bash
cd your-project      # now contains lns.yaml + config/
lns run
```

The first boot runs `npm install -g @anthropic-ai/claude-code`; the microVM is
ephemeral, so each cold start reinstalls it (the `allowedRoutes` keep
`registry.npmjs.org` open for that reason).

On first run, an approval card asks for your Claude subscription token and shows
how to mint it.

## Notes

- The network allowlist is deliberately tight. `registry.npmjs.org` is only
  needed for the first-boot install; if you bake Claude Code into a custom image
  and `lns push` it, you can drop that route and the runtime install entirely.
- Everything except `/workspace` is ephemeral. Session history and any config
  Claude writes at runtime die with the microVM; the seed `config/` is one-way
  (host → guest). Attach a named volume if you want state to persist.
