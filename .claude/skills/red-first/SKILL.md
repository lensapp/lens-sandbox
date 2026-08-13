---
name: red-first
description: Red-test-first fix loop for bugs and behavior changes — state the defect as a claim, write the failing test before any production code, watch it fail for the stated reason, then make the smallest change that turns it green. Includes follow-up sweeps (coverage of new properties, side-effect accounting, fixture reality checks), mutation proofs for green-on-arrival tests, and a strictly bounded, optional end-to-end phase used only when a contract crosses a component boundary. Use this whenever the user reports a bug, asks to fix an issue, asks whether a finding is real and then wants it fixed, asks to verify a fix with tests, or asks to harden tests against regressions — even if they don't say "TDD" or "red test".
argument-hint: "[issue description] [optional: suspected cause or proposed fix]"
---

# Red-First Fix Loop

Fix bugs by writing the failing test before the fix. The failure message is the
specification; the test outliving the fix is the regression guard. Every loop
below has the same shape: claim → red → green → gate → commit.

## Arguments

`/red-first <issue description> [suspected cause or proposed fix]`

- **Issue description given:** distill it into the one-sentence claim of loop
  step 1. If the description is a question ("is this finding real?"), first
  verify the finding against the code and report; enter the loop only once the
  defect is confirmed (or the user has asked for the fix).
- **A suspected cause or proposed fix given:** treat it as a hypothesis, never
  as permission to skip steps. The red test still comes first, and it must fail
  for the *claimed* reason — if it doesn't, the hypothesis is wrong and that is
  worth reporting before any code changes. Prefer the user's proposed direction
  when it survives the red test, but say so explicitly when the evidence points
  elsewhere.
- **No arguments:** take the issue from the conversation context; if there is
  none, ask for a description of the defect (what happens, what should happen,
  how to reproduce if known).

## The core loop

1. **State the defect as one claim.** One sentence: what the code does wrong,
   and what correct looks like. If you cannot write this sentence, you don't
   understand the bug yet — investigate first, fix second.
2. **Pick the lowest test layer that can express the claim.** Follow the
   repository's own test-layer conventions (check CLAUDE.md or equivalent).
   Unit/behavioral layers are the default. Reach for end-to-end only under the
   conditions in "The optional end-to-end phase" below.
3. **Write the test. Write no production code yet.** If the fix will create a
   new function or trait method, stub it to compile with wrong behavior (return
   the wrong value), so the failure is behavioral, not a compile error.
4. **Run the test and read the failure.** It must fail, and it must fail for
   the stated reason. A test that fails differently has found a different
   problem — stop and examine before proceeding. A test that passes means the
   claim is wrong or the test is aimed at the wrong code.
5. **Write the smallest change that makes it green.** Queue any larger redesign
   as its own loop with its own red test — don't fold it in.
6. **Run the repository's full verification gate, then commit.** Small commits
   keep each loop reversible. Use the repo's commit conventions.

## Follow-up sweeps (each one starts a new loop)

Run these after the first green. Each finding becomes a new red test.

**Coverage of new properties.** The fix created properties no test pins yet:
orderings and precedences between data sources, recovery paths after state
loss, and any consciously accepted risk boundary. Pin each one. Record accepted
trade-offs as tests whose one-line comment states the decision and its
consequence — so a future change to that behavior is a visible decision, not
an accident.

**Side-effect accounting.** For each behavior change, name who it can hurt and
where the failure moved (e.g., "error moves from producer to consumer",
"validation moves from commit time to run time"). "No harmful side effects" is
never the honest summary — the honest form is "these costs, accepted because
X". A cost you can't accept is the next loop's claim.

**Fixture reality check.** A contract test is only as strong as its fixture.
When a claim is universal ("every X satisfies Y"), fetch real-world data before
trusting it. If reality falsifies the claim, fix the *claim*, not the data —
then check the real data in as a fixture with a documented refresh mechanism,
so future drift turns a test red at a controlled moment instead of in
production.

## Prove that green tests can fail

A test written after the fix is green on arrival and proves nothing yet.
Prove sensitivity by mutation: break the production code in exactly the way
the test guards against, run the test, watch it fail, restore the code. Do this
at least for the load-bearing test of each loop. Two blind spots to remember:
mutation cannot see inputs no fixture supplies (needs real-world data), and it
cannot see a false assumption shared by two test suites (needs a
cross-boundary test).

## The optional end-to-end phase

**Skip this phase entirely unless the fix spans a producer/consumer boundary** —
two components that must agree on a contract (a wire format, a published
artifact one side writes and the other reads, a cache key derived on both
sides). Each side's suite proves its half against its own fixtures; only a test
that produces the artifact and then consumes it proves the halves agree. If the
fix lives inside one component, the lower layers are sufficient — stop here.

When the phase is justified, keep it strictly bounded:

- **One scenario, smallest viable fixture.** Prefer the cheapest inputs that
  exercise the contract (tiny downloads, smallest images). Verify platform
  compatibility of fixtures before running (does this artifact exist for this
  OS/arch?).
- **Design around the failure condition, not the happy path.** Remove the
  crutch — delete the cache/record, cut the network — and assert the system
  still works from the durable contract alone.
- **Guard every setup step.** Assert exit codes, assert a file exists before
  deleting it, refuse warnings in output that should be clean. A silent setup
  failure makes the scenario pass for the wrong reason; a guard turns it into a
  readable error that names the real problem.
- **Timebox every run and run it in the background.** Decide the expected
  duration before starting (ask: what real work must happen — downloads, VM
  boots?). If a run exceeds ~2× that estimate, do not keep waiting: kill it and
  switch to measurement.
- **Diagnose by measurement, not by waiting.** In order of cost: does the
  output/target grow (bytes moving)? are network connections open? do a simpler
  request and a guaranteed-fast-failure request behave differently? If samples
  show nothing, rebuild with debug symbols and take a stack sample — one
  symbolized stack usually ends the search. Distrust any theory that explains
  only some of the symptoms.
- **Expect unplanned failures and treat each as a finding.** An end-to-end
  scenario often fails first in code nobody has run since it landed. Fix each
  finding with its own core loop (red at the lowest layer that can express it),
  then rerun the scenario.
- **Sensitivity proof is optional here.** If a mutation proof needs expensive
  rebuilds, and the scenario has already demonstrated it can fail (it failed
  during bring-up), rely on that plus the lower-layer pins — say so explicitly
  in the report instead of silently skipping.

## Reporting

End every loop with: the claim, where the red test lives, proof it was red
(the failure line), what changed, and gate status. End the session with a
loop-by-loop summary and an explicit list of accepted trade-offs and anything
skipped (e.g., an e2e mutation proof). Report failures faithfully — a red gate
or a skipped step is stated, never smoothed over.
