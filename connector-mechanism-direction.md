# Connectors: a `code` auth kind that brings its own implementation

## The decision

`auth.kind` keeps its declarative kinds — `token` today, `oauth_device` and `oauth_pkce`
when the deleted engine comes back — and gains one more:

```yaml
methods:
  - name: sign-in
    auth:
      kind: code
      component: ./sign-in.wasm  # a file beside the document, packed at publish
      outputs: [token]        # what connect returns, so credentials[].field validates offline
      hosts: [auth.example.com, api.example.com]
      limits: { callSeconds: 30, sessionSeconds: 900 }
      # exec: true            # opt-in, and it changes what the card can promise
```

The component implements three functions: **`connect`**, **`revoke`**, **`refresh`**. lns
decides when each runs: a person triggers `connect` and `revoke`, and lns schedules
`refresh`. Nothing else about the connector changes.

This is one mechanism, not two spellings: a `kind` names how a value is obtained, and
`code` means "the author's implementation" where `token` means "ask the user" and `oauth_device`
means "a descriptor lns interprets."

## Why this shape

The argument was never about capability — descriptors can express every provider we have
met. It is about **where complexity accumulates**. Each odd provider makes the descriptor
path grow a field: a `verification_url` alias for Google, `jsonField` pointers, a transform
set for auth files, `hostSource` for the Keychain. Three function references never grow.

So the split is: the common case stays five lines of YAML that a user can read and diff,
and the long tail stops blocking on our release cadence. That second half is what makes
this worth building even before anyone uses it — it changes what we can promise from "the
providers we support" to "any provider".

## What does not change

Supply stays declarative, and that is most of the document: `serves`, `credentials`,
`injections`, `egress`, and filesets. A `code` method obtains a value; how that value
reaches a workload is decided the same way for every kind, and §3.2.5 still holds — a
fileset carries the placeholder, never the value.

So `code` replaces the auth block, not the connector.

## The three functions

**`connect` and `revoke` run only when a person presses a button** — Connect, or
Disconnect. Never on a schedule, never in the background, never on workload behaviour. One
consequence matters: a guest can never choose when host code runs, so there is no timing
channel out of the sandbox.

**`refresh` is the exception: it runs as a background poll on lns's schedule.** A token
expires on the provider's clock, not on a button press, and a run that outlives its token
is the case that matters. So one function does run unasked. The bound that survives is that
**lns owns the schedule** — the component cannot ask to be woken, cannot set its own
interval, and every call is audited.

| Function | lns runs it | In | Out | Deadline | On trap or timeout |
|---|---|---|---|---|---|
| `connect` | the user pressed Connect | `begin(inputs)`, then `resume(state, event)` for user input, a timer within the session, or a callback | `ask` \| `wait{millis}` \| `done{values, scopes, account}` \| `failed` | 30 s per call, 15 min per session | connect fails; the offer stands |
| `refresh` | a background poll on lns's schedule | `values` | new `values` | 30 s | retried per a backoff that is **not yet decided** (see below); the connection is then marked stale and the next use raises the card |
| `revoke` | the user pressed Disconnect, or uninstalled | `values` | `()` | 10 s | logged; connection dropped anyway |

### The open question: what schedule

Decided: a background poll, and lns owns it. Undecided: the policy.

Four facts shape it, each checked:

- **Nothing we inject *today* expires — but the class this feature exists for is expiring
  by nature.** GitHub PATs and OAuth App tokens, Anthropic and OpenAI API keys: no expiry.
  Expiry exists for OAuth *access* tokens (1–8 h), AWS sessions, and every credential a
  host tool owns and renews. Measured on a real machine, Claude Code's own store carried an
  access token good for about two hours and a refresh token good for about nine days. So
  the poll does not matter for the `token` kind and matters for almost everything a
  `kind: code` connector will bring.
- **Today, an expired value fails silently.** The proxy holds a request only when a
  placeholder is *unarmed*; an armed-but-expired value is injected and the upstream 401 is
  relayed to the workload unchanged — `proxy.rs:306` is `intercept_for_unarmed`, and
  neither `proxy.rs` nor `mitm.rs` mentions 401 at all. No card, no hold, no
  retry. The old engine's only recovery — drop the grant, unarm the placeholder, let the
  next request raise the prompt — went out with the deletion.
- **Nothing bounds a run.** No idle sweep, no maximum lifetime; a detached run can live for
  days. So "the run outlives its token" is ordinary, not an edge case.
- **The mid-run re-arm path already exists.** Grants re-send a payload through the
  policy-change frame; a refresh can ride it.

What has to be answered before this is buildable —

- **What drives the interval.** An expiry the component reports back with its values, or a
  fixed cadence lns picks. Expiry-driven is the obvious answer and it means the component
  influences timing, so it needs a floor: a component reporting a one-second lifetime must
  not produce a hot loop.
- **Whether an idle machine polls.** The tray runs for weeks. Polling a connector no run has
  used since Tuesday costs nothing to us and tells the provider our user's machine is awake.
  The alternative — poll only while a run holds the connector — leaves a token stale at the
  moment a run starts, which pushes work back to run start.
- **Sleep and wake.** A closed laptop misses every tick. On wake, everything is overdue at
  once; that needs jitter, not a thundering herd.
- **Failure policy.** How many failed refreshes before lns stops calling and marks the
  connection as needing a reconnect. A provider outage must not burn the grant.
- **What the user can see and stop.** A background call is invisible by definition, so the
  ledger entry and some surface in the tray are part of the feature, not polish. The store
  for it already exists and needs no design: `ledger::append_machine_event` writes the
  **durable ledger** (`~/.lns/ledger.jsonl`, hash-chained against `ledger.anchor`, serialized
  by a global lock), which `lns audit` already merges with each run's own chain. Its
  precedent is exact — `append_tool_provisioned` puts tools fetched during a pull there,
  "because they belong to no run". A connection belongs to no run either, which is why
  `ConnectorVerb` deliberately records only what a *run* decided. What is missing is not a
  facility but the entries: an outbound call, a program started, a scheduled renewal — and
  a `lns audit` kind that can select them, or they are written where no user looks.

**Failure handling is not part of that question, and should land regardless.** When a
refresh fails — or for any credential with no `code` behind it — lns should read the
declared expiry and **unarm** the placeholder at expiry. No code runs; the next request is
held and raises the card, which is the behaviour the spec already describes. That turns
today's silent 401 into a prompt, works for the declarative kinds too, and is the backstop
the poll needs when a provider is down.

The rest of this note does not depend on the answer.

`connect` is a step machine rather than one call because a device flow has to prompt and
wait. `done` carries the account label with the values, so a `code` connector gets the
same connection identity a descriptor one does.

One thing stays out of the component: it is never in the request path — injection and
signing stay in the guest supervisor.

## Host execution, and what it costs

An earlier draft of this note also kept host *reads* out of the component: the manifest
would declare each source, lns would read it, and the component would receive bytes. That
is not enough, and the case that breaks it is the one that motivates the whole feature.

**A credential is often owned by a tool that already knows how to renew it.** `gh auth
token`, `gcloud auth print-access-token`, `aws configure export-credentials`, `claude`. The
ecosystem has converged on this three separate times — Kubernetes `client.authentication.k8s.io`
exec-credential plugins, `docker-credential-*` helpers, `git credential-*` helpers — each
one "run a binary, take a credential and an expiry, run it again when that expires".

A declared *source* cannot express that. Reading Claude Code's store means a Keychain on
macOS and a file on Linux, so a `hostSource` needs two source kinds and a platform branch;
`claude auth status --json` is one invocation on both. **The command is the stable
interface; the store is not.**

So the component may run a host program, and it composes the invocation itself. Two
consequences, both taken deliberately:

- **The sandbox's bounds become advisory for a component that has this capability.**
  Declared hosts, no filesystem, no environment, no clock, fuel — a component that can run
  `curl` has all of them back. The bounds still hold for a component without it, and they
  are what the sandbox endgame below restores. But for one with it, the enforcement layer
  is the user's own account, and the card must say so rather than claim otherwise.
- **The later move into a sandbox is a breaking change, not a migration.** A component
  composing `/usr/local/bin/claude` finds a different filesystem inside a sandbox. Every
  component that exists by then is rewritten. That is cheap now, while they are all
  first-party, and it is the window.

**What holds the line instead of the sandbox is provenance.** For this phase, a `code`
method that declares host execution may be installed only from a local path; a pulled
artifact carrying one is refused. Later that becomes a trusted-publisher gate — the
publisher signature stops being informational and becomes the thing carrying the trust the
sandbox cannot. Either way it is one rule: **something other than the digest vouches for
the code, because the digest cannot.**

**The endgame is to run the execution inside a sandbox**, which is more on-product than
this note's original answer. This note rejected a microVM because it "gives the code a
filesystem and a clock we would then have to take away" — true for an author's logic
component, and exactly inverted here: a filesystem and a clock are what `claude` needs to
read a credentials file, refresh, and write it back. A sandboxed refresh also gets lns's
own egress policy applied to it, which is the product enforcing its own principle on
itself.

## The layering: `code` is the primitive

`token`, `oauth_device` and `oauth_pkce` are **built-in mechanisms implementing the same
three functions**, bundled in lns. `kind: code` is the same interface with the author's
implementation behind it. One interface, several adapters.

`token` fits: `begin` returns a prompt, `resume(user_input)` returns `done`, no I/O and no
declared hosts. Device flow fits: `begin` POSTs device authorization and returns
`prompt{url, code}`, then `wait`, then polls — `authorization_pending` returns another
`wait`, `slow_down` returns `wait{interval + 5s}` (RFC 8628 §3.5), and the interval lives in
the mechanism's state while the host only sleeps.

**The built-ins are native Rust, not bundled components.** Making Wasmtime load-bearing for
a token paste is disqualifying: size, init cost on the most trivial path, and no connector
connects at all if the runtime fails to start. The uniformity that argues for real
components is bought more cheaply — built-ins call through the **same host fetcher and the
same host filter**, so an outbound call produces an identical audit row whichever adapter
made it, and the filter is exercised on every connect rather than only on the rare one.

**The discipline that makes "the same API" true rather than aspirational:** define the
interface once as the WIT world, have the native adapters implement exactly that world and
nothing more, and run one conformance suite against both. A built-in that needs something
outside the world means the world is wrong — fix the world, never special-case.

### The world, sketched

Not final — a sketch to argue with, and to check that everything the walk-through found has
somewhere to live.

```wit
package lns:connector@0.1.0;

interface types {
  type values = list<tuple<string, string>>;

  record account { id: string, display-name: string }

  // What the card asks for. A method may produce two values, so this is a
  // field list, not a string, and each field says whether it is secret.
  record field { name: string, label: string, help: option<string>, secret: bool }
  record ask   { text: string, url: option<string>, code: option<string>, fields: list<field> }

  record renewal { values: values, scopes: list<string>, account: option<account> }
  record failure { reason: string, retryable: bool }

  variant step {
    ask(ask),                 // show this, then call resume with input
    wait(u64),                // sleep this many millis, then call resume with tick
    done(renewal),
    failed(failure),
  }

  // The component is a state machine the host drives; state is opaque bytes
  // the host holds in memory between calls and never persists.
  record progress { state: list<u8>, step: step }

  variant event {
    input(values),            // the user answered the ask
    tick,                     // the wait elapsed
    callback(string),         // raw query string from the loopback redirect
  }

  // Everything ambient arrives here. No clock, no environment, no filesystem.
  // A component that needs the host's own credential store runs the tool that
  // owns it (see `exec`) rather than being handed bytes lns read for it: the
  // store differs per platform, the command does not.
  record context {
    now-millis: u64,
    inputs: values,                              // descriptor fields: client id, endpoints, scopes
  }
}

interface http {
  record request  { method: string, url: string, headers: list<tuple<string, string>>, body: option<list<u8>> }
  record response { status: u16, headers: list<tuple<string, string>>, body: list<u8> }
  // Refused for any host outside the manifest's `hosts`. Every call is an audit row.
  fetch: func(req: request) -> result<response, string>;
}

interface entropy {
  bytes: func(n: u32) -> list<u8>;
}

interface callback {
  record binding { redirect-uri: string, state: string }
  // The host binds the loopback listener, owns the port, and checks the state
  // nonce before it ever calls resume. Refused unless the manifest asks for it.
  bind: func() -> result<binding, string>;
}

interface exec {
  record output { status: s32, stdout: list<u8>, stderr: list<u8> }
  // The component composes the argv. lns bounds nothing about what the program
  // then does — it runs with the user's own access. Refused unless the manifest
  // asks for it, refused for a pulled artifact, and every call is a ledger row.
  run: func(argv: list<string>) -> result<output, string>;
}

world mechanism {
  import http;
  import entropy;
  import callback;
  import exec;

  export begin:   func(ctx: context) -> progress;
  export resume:  func(state: list<u8>, ev: event, ctx: context) -> progress;
  export refresh: func(held: values, ctx: context) -> result<renewal, failure>;
  export revoke:  func(held: values, ctx: context) -> result<_, failure>;
}
```

What each import is bounded by, and by whom:

| Import | Bound | Enforced by |
|---|---|---|
| `http.fetch` | the manifest's `hosts`, TLS only | host, per call, audited |
| `entropy.bytes` | a byte ceiling | host |
| `callback.bind` | one listener per session, host-owned port, host-checked `state` | host |
| `exec.run` | **nothing about the program itself.** Local-install only, declared on the method, every call a ledger row | provenance, not the sandbox |

Deliberately absent, each for a reason already argued: no clock (`now-millis` is an input,
so a mechanism cannot time anything the host did not tell it); no filesystem and no
environment **as imports of their own** — a component that needs either reaches them only
through `exec`, where the capability is declared, disclosed, and gated on provenance, so
there is one such door rather than three; no `cancel` export — the host stops calling
`resume` and drops the state; and no way for a component to request a wake-up, which is
what keeps the refresh schedule lns's to own.

Two things the sketch makes obvious that prose hid. `refresh` returns a `renewal` exactly
like `done`, so the host's carry-forward rule — keep the previous scopes and account when a
response omits them — applies to one type in one place. And `progress.state` is secret
material (a device code, a PKCE verifier), so the host holding it in memory with a bounded
lifetime is part of the world's contract, not an implementation detail.

### What the walk-through exposed

Five things the interface must carry, found by putting the real mechanisms through it:

- **The ask needs a field list with a secret flag**, not just text — §3.2.2 already lets one
  method produce two values, and the CLI renders the ask from the auth's label and help.
  This is why the sketch's step is `ask(fields)` rather than a string.
- **PKCE needs an inbound capability**: bind a loopback listener, put its port in the
  authorization URL, receive one browser request, check the `state` nonce. That is an
  import and an event, not a step. The uncomfortable part is that the API *can* express it,
  which means a `code` connector may ask for a loopback listener too — new attack surface,
  and the nonce check has to be host-enforced rather than left to the component.
- **Cancel is not an export.** The host stops calling `resume` and drops the state; the host
  owns cleanup of the listener. Pivoting to another method is the card choosing differently,
  outside the world entirely.
- **Identity rides in `done{values, scopes, account}`**, and `refresh` returns the same
  shape. When a refresh response omits scopes or account, the **host** carries the previous
  ones forward — one rule, applied to third-party components too.
- **Authority canonicalisation is host work.** The mechanism reports scopes; the host builds
  the canonical set, or a component could report an order-dependent list and defeat §3.2.4.

`client_secret` as an input stays what it is: a secret in a public document. The layering
does not fix it and should not pretend to.

### The asymmetry, stated

Same API at the interface; **not** the same at the enforcement layer. A component's bounds
are enforced — declared hosts, no filesystem, fuel, deadlines. A built-in's bounds are a
code review. That is the normal posture for a plugin host, and it holds as long as two
things are true: built-ins run under the same host filter, and the card says who vouches.
For a built-in: "lns signs in with `oauth_device` at `<token_endpoint>`" — a mechanism the
user can read about in the spec. For `kind: code`: "this connector's own code signs in; it
may contact X and Y; lns cannot show what it does."

### What the layering costs

- The world **freezes the prompt vocabulary**. A new card affordance — a QR code, an "open
  browser" button — becomes a WIT version bump every component must follow.
- The world is **the first API we cannot break pre-1.0**. Third-party artifacts are not ours
  to recompile, so CLAUDE.md's no-shims rule meets its first real exception the day one
  exists. That is the strongest reason to validate the world hard before publishing it.
- Step state — a device code, a PKCE verifier, a nonce — is **secret material the host holds
  between calls**. In memory only, with a lifetime the host bounds.
- The inbound loopback capability is new attack surface for every `code` connector that
  asks for it.
- Expressing a token paste as a serialisable step machine is ceremony, and two adapters can
  drift. The conformance suite is what keeps the second one honest.

## Where the code runs

On the host, in `lns-service`, as a WASI component under Wasmtime. Not in the supervisor,
and not in a microVM — a VM costs seconds and hundreds of MB per connector, and gives the
code a filesystem and a clock we would then have to take away.

Bounds, enforced by the host and never trusted from the component: outbound HTTPS only to
the declared `hosts`, each call written to the durable ledger; no filesystem, no
environment, no clock (`now` is an input); `random` by import; fuel, memory, and
component-size ceilings; the deadlines above.

**Every one of those bounds is advisory for a component that declares `exec`**, because a
program it starts is subject to none of them. That is stated here rather than buried: the
list above describes a component without the capability, and provenance rather than the
sandbox is what bounds one with it.

Cost to accept: roughly 20–30 MB of Wasmtime in `lns-service` (unmeasured on our release
profile), a cross-build it has not seen, and an authoring toolchain — Rust, or
`componentize-js` — for people who today write YAML.

## What the card must say

A declarative connector can be read before it is trusted. A component cannot, so the card
shows bounds instead of behaviour: the hosts it may contact, the publisher signature, the
digest, and one plain sentence — **"lns cannot show what this code does. It can only bound
where it runs, what it reaches, and how long it has."**

**A component that declares `exec` gets a different sentence, because that one would be
false.** The second half is the part that stops being true: lns bounds where it runs and
how long it has, but not what it reaches. So the card says instead — **"lns cannot show
what this code does, and it runs programs on your machine with your own access. lns cannot
bound what those reach."**

Both sentences are the honest price of the feature, and they are the reason `token` and the
OAuth kinds should stay declarative rather than being reimplemented on top of `code`. The
difference between them is also the reason `exec` is a separate declaration rather than
something every `code` method carries: a component that does not ask for it earns the
better sentence.

## Spec changes

These land together rather than one per section, because split apart the earlier
commits would reference a §3.2.6 that does not exist yet. They are one decision — the
kind exists and may execute — and the sections below are its consequences.

- **§3.2.3** — narrow, do not reverse. "Connecting a connector MUST NOT run code **in the
  guest**. Code a `code` method carries runs on the host, in the functions §3.2.6 names,
  under the bounds it declares." The `scripts` row keeps its `MUST NOT`; only its rationale
  narrows.
- **New §3.2.6** — the `code` kind: the three functions, their inputs and returns,
  deadlines, trap semantics, the capability block, and the timing rule in both its halves —
  a press for `connect` and `revoke`, an lns-owned schedule for `refresh`. The capability
  block is where `exec` is defined, along with the two rules that bound it: local-install
  only, and a card sentence that claims nothing about what it reaches.
- **§1.3** — a **program** on the running machine joins the three things outside a
  document's reach. It fits the existing pattern rather than reversing it: a pulled document
  only names it, and whether it runs is decided per machine, on the same terms as a
  destination.
- **§3.2.2** — `code` joins the kind table. A `code` method MUST declare `outputs[]`,
  because a component produces whatever it says it produces; §3.2.2's `Produces` column and
  §4.1's `field` rule then validate a credential's `field` against that list. Without it
  `lns artifact validate` cannot check a `code` connector at all.
- **§1.5** — the card's code paragraph is part of the one disclosure.
- **§5** — validation rows: a `code` block on any other kind; unknown function names; ceilings.
- **§7.1** — a component layer media type beside the fileset layer, and §6 a publish-time
  transform that packs it. `component` keeps its spelling across the push, exactly as a
  fileset `path` does, so there is no second digest to write down; the artifact digest a
  grant already binds to covers the component's bytes, and digest binding already covers
  re-consent. The installed set has to capture those bytes too, and digest over them: a
  local install has no artifact, and an `exec` component can only ever arrive that way, so
  without it an author could swap the file under an installed connector and every grant
  would stand unasked.

## What to watch

The reason to keep this an escape hatch rather than the main path:

- **No provider needs it yet.** GitHub, Google and Claude Code all express as descriptors.
  That is fine for a hatch — it is supposed to be rare — but it means the runtime is paid
  for before it is used.
- **The trivial case must not drift into it.** If a five-line `token` connector ever ships
  as a component, the user is consenting to a blob for no reason.
- **Authoring.** Connector contributions are YAML today; a component turns a ten-minute
  contribution into a build pipeline. Expect first-party authors only, for a while.
- **Wasmtime cadence.** Its release and CVE rhythm becomes ours.
- **`refresh` is the hole in the press-only property**, taken deliberately. Watch that it
  stays one function on a schedule lns owns, and does not become a general "the component
  may run periodically" facility.
- **`exec` is the hole in the bounded-component property**, also taken deliberately. Watch
  two things. That a component asks for it only when driving a host tool is the point —
  a component reaching a network through `exec` rather than `http.fetch` has escaped the
  host filter, and the card cannot tell the user which happened. And that "local-install
  only" does not quietly relax before the trusted-publisher gate that replaces it exists;
  the day a pulled artifact can carry `exec`, provenance is the only bound left and it must
  actually be there.

## First slice

Two reorderings, each for its own reason.

**The spec decisions come first, before any code.** CLAUDE.md's transitional rule is
explicit that a change may not add a field the specification does not describe, and
`auth.component`, `auth.outputs`, `auth.hosts` and the `exec` capability are exactly that.
So §3.2.6, the §3.2.3 narrowing, the §1.3 addition, and the §3.2.2 kind row land as
decision commits ahead of the grammar that implements them — not, as an earlier draft of
this note had it, alongside.

**Wasm comes before the native adapters**, which inverts what this section first said. The
reasoning that put native adapters first — that three real mechanisms would validate the
world — does not hold: a native adapter allocates freely, blocks, and holds state a
component cannot, so it would validate nothing about whether the world is implementable by
a component. Only a component validates a world for components. The guard that keeps the
inversion honest is that **the world does not freeze until a native adapter fits it
unchanged**, or it ends up Wasm-shaped and the built-ins cannot implement it.

1. The spec decision commit above.
2. The `code` kind in the connector grammar: `component`, `outputs`, `hosts` and the `exec`
   declaration, each refused on any other kind, and a credential's `field` validated against
   the declared `outputs` rather than against a table this version holds.
3. Define the world, and the `Mechanism` port from it — including the inbound callback
   capability PKCE forces, so the world is not wrong on arrival, and `exec`.
4. The Wasmtime adapter behind that port. Layer 3: the host filter refuses an undeclared
   host, a call past its deadline traps, state round-trips through `resume`, `now` arrives
   as an input. Fixture component built for `wasm32-wasip2` in CI.
5. One real component end to end, not a fixture — the host-tool case, which is what proves
   `exec` and the whole premise.
6. Native adapters for `token`, `oauth_device` and `oauth_pkce`. Three, not two — PKCE is
   what forces the inbound callback capability. **This is the freeze point:** if the world
   has to change to fit a native adapter, it changes here, before anything external exists.
7. One conformance suite, run against every adapter: the `oauth_connector.feature` and
   `pkce_connector.feature` deleted in `d927b598`, rewritten against the port, plus a
   `token` feature pinning today's `handler::connect` behaviour.
8. Layer 2 through the mocked port: a `code` method's card shows its hosts and digest and
   the sentence its capabilities earn; a failed `connect` leaves the offer standing;
   `revoke` drops the connection even when it traps.

## Appendix: what the investigation found

Kept because it decides what `code` should *not* be used for.

- **The deleted OAuth engine was already declarative**, refresh included
  (`d927b598^:crates/lns-service/src/oauth/traits.rs`, `real.rs:207` is RFC 6749 §6).
  Restoring it is undeleting ~1.9k lines.
- **A `prepare` function would not earn its place.** Every real auth file — Claude Code,
  Codex, gh, git, docker, npm, pip, Vault, Terraform, AWS, gcloud, kubeconfig, Gemini —
  renders from a template plus a closed transform set (`base64`, `urlencode`,
  `json_string`, `now(+offset)`, a JWT claim read). The paths must be static anyway,
  because the boot counts what installed connectors write
  (`crates/lns-service/src/connector/real.rs:345`), and code output would make §3.2.5
  unenforceable. Binary stores (SQLite, gpg `trustdb.gpg`, Java keystores) defeat a
  no-filesystem component just as thoroughly.
- **Refresh, parked.** "Refresh differs per provider" is not the problem; ownership of a
  rotating secret across the host/guest boundary is. The shipped Claude seed carried a
  *placeholder* refresh token with `expiresAt: 4102444800000` (year 2100) — that value is
  the **seed's**, not a real store's, which is why it reads as absurd; a real machine's
  store carries an ordinary few-hour expiry. So the guest never refreshes and the host copy
  goes stale in an hour; the Codex recipe (not in the tree —
  `73cb1a7a:docs/examples/codex-chatgpt-subscription/lns.yaml`, on the `codex-example`
  branch) uses no connector and keeps a live refresh token in a named volume. The answer is a
  `kind: token_exchange` injection letting the proxy hand a refresh POST to the host. A
  `refresh` function serves host-held credentials; it does not touch that case.
- **We already ship MCP tool gating and never described it.** Pinned core (`71e3c1c1`) has
  `HttpRule.mcp: Option<McpMatcher>` with method and name globs, tests gating `tools/call`
  by tool name, and a 1 MiB judged-body ceiling; §4.2 exposes none of it. A method with an
  `mcp` matcher plus a bearer injection turns "a GitHub token" into "these eight tools",
  disclosed on the card, with no new runtime. Separate work, and on current evidence a
  better buy than anything above. (Check this in the pinned checkout, not a local
  `lens-sandbox-core` branch — mine was on one whose HEAD lacks the pinned rev, which is how
  I first reported it wrongly.)
