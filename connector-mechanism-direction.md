# Connectors: a `code` auth kind that brings its own implementation

## The decision

`auth.kind` keeps its declarative kinds — `token` today, `oauth_device` and `oauth_pkce`
when the deleted engine comes back — and gains one more:

```yaml
methods:
  - name: sign-in
    auth:
      kind: code
      component: sha256:…     # a component layer in the connector artifact
      outputs: [token]        # what connect returns, so credentials[].field validates offline
      hosts: [auth.example.com, api.example.com]
      limits: { callSeconds: 30, sessionSeconds: 900 }
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

- **Nothing we inject today expires.** GitHub PATs and OAuth App tokens, Anthropic and
  OpenAI API keys: no expiry. Expiry exists only for OAuth *access* tokens (1–8 h) and AWS
  sessions. So the poll matters for the OAuth kinds and for whatever `kind: code`
  connectors bring — not for the `token` kind.
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
  audit row and some surface in the tray are part of the feature, not polish.

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

Two things stay out of the component. It is never in the request path — injection and
signing stay in the guest supervisor. And **it never performs a host read itself**: the
manifest declares each source (a Keychain service, or a home-relative file under the
refusals `413d37b2` implemented and `d927b598` deleted along with the rest of the connector
layer — so they are a thing to rebuild, not a thing to reuse), lns reads it, and the
component receives bytes.

That is what lets the Claude Code case become a `code` connector rather than a declarative
one. The component gets the Keychain blob and does whatever extraction the tool's format
needs; lns still decides what may be read, and the card still names it. Detection needs no
separate function: the user presses Connect, the component inspects the blob it was handed,
and returns `done` immediately instead of prompting.

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
  record context {
    now-millis: u64,
    inputs: values,                              // descriptor fields: client id, endpoints, scopes
    host-blobs: list<tuple<string, list<u8>>>,   // declared host sources, read by lns
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

world mechanism {
  import http;
  import entropy;
  import callback;

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

Deliberately absent, each for a reason already argued: no clock (`now-millis` is an input,
so a mechanism cannot time anything the host did not tell it); no filesystem and no
environment; **no host-secret read** — lns reads each declared source and passes the bytes
in `host-blobs`; no `cancel` export — the host stops calling `resume` and drops the state;
and no way for a component to request a wake-up, which is what keeps the refresh schedule
lns's to own.

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
the declared `hosts`, each call written to the audit chain; no filesystem, no environment,
no clock (`now` is an input); `random` by import; fuel, memory, and component-size
ceilings; the deadlines above.

Cost to accept: roughly 20–30 MB of Wasmtime in `lns-service` (unmeasured on our release
profile), a cross-build it has not seen, and an authoring toolchain — Rust, or
`componentize-js` — for people who today write YAML.

## What the card must say

A declarative connector can be read before it is trusted. A component cannot, so the card
shows bounds instead of behaviour: the hosts it may contact, the publisher signature, the
digest, and one plain sentence — **"lns cannot show what this code does. It can only bound
where it runs, what it reaches, and how long it has."**

That sentence is the honest price of the feature, and it is the reason `token` and the
OAuth kinds should stay declarative rather than being reimplemented on top of `code`.

## Spec changes, each its own decision commit

- **§3.2.3** — narrow, do not reverse. "Connecting a connector MUST NOT run code **in the
  guest**. Code a `code` method carries runs on the host, in the functions §3.2.6 names,
  under the bounds it declares." The `scripts` row keeps its `MUST NOT`; only its rationale
  narrows.
- **New §3.2.6** — the `code` kind: the three functions, their inputs and returns,
  deadlines, trap semantics, the capability block, and the timing rule in both its halves —
  a press for `connect` and `revoke`, an lns-owned schedule for `refresh`.
- **§3.2.2** — `code` joins the kind table. A `code` method MUST declare `outputs[]`,
  because a component produces whatever it says it produces; §3.2.2's `Produces` column and
  §4.1's `field` rule then validate a credential's `field` against that list. Without it
  `lns artifact validate` cannot check a `code` connector at all.
- **§1.5** — the card's code paragraph is part of the one disclosure.
- **§5** — validation rows: a `code` block on any other kind; unknown function names; ceilings.
- **§7.1** — a component layer media type beside the fileset layer. Digest binding already
  covers re-consent.

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

## First slice

The layering reorders this: **build the port and the native adapters first, and add
Wasmtime only once three real mechanisms have validated the world.**

1. Define the world, and the `Mechanism` port from it.
2. Native adapters for `token`, `oauth_device` and `oauth_pkce`. Three, not two — PKCE is
   what forces the inbound callback capability, and a world validated without it will be
   wrong the day the first `code` connector needs a browser redirect.
3. One conformance suite, run against every adapter: the `oauth_connector.feature` and
   `pkce_connector.feature` deleted in `d927b598`, rewritten against the port, plus a
   `token` feature pinning today's `handler::connect` behaviour.
4. Only then the Wasmtime adapter, behind the same port. Layer 3: the host filter refuses an
   undeclared host, a call past its deadline traps, state round-trips through `resume`,
   `now` arrives as an input. Fixture component built for `wasm32-wasip2` in CI.
5. Layer 2 through the mocked port: a `code` method's card shows hosts, publisher and
   digest; a failed `connect` leaves the offer standing; `revoke` drops the connection even
   when it traps.
6. Spec §3.2.6 and the §3.2.3 narrowing land as decision commits alongside.

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
  *placeholder* refresh token with `expiresAt: 4102444800000` (year 2100), so the guest
  never refreshes and the host copy goes stale in an hour; the Codex recipe (not in the tree —
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
