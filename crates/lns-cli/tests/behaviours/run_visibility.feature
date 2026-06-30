Feature: users see what `lns run` is doing from the moment they hit Enter
  `lns run <image>` can take several seconds on a cold cache — image
  resolve, layer pull, composefs build, microVM boot, broker handshake.
  Today that gap is silent: the operator cannot tell what policy they
  are about to trust, what resources or flags are in effect, or whether
  work is progressing. The `docs/overview.md` Quickstart already shows
  the intended UX (an up-front summary plus per-phase `✓` lines); these
  scenarios pin that UX as the acceptance bar.

  Background:
    Given the user invokes `lns run ubuntu` from an interactive terminal
    And the working directory is `~/dev/my-app`

  Scenario: Summary block appears immediately on invocation
    When the command starts
    Then a run summary is printed before any service round-trip
    And the summary lists Image, Resources, Flags, and a Policy block
    And fields not yet known to the CLI are shown as "(resolving…)"

  Scenario: Policy block answers what and why
    Given the working directory contains `./lns-policy.yaml`
    And that policy has default verdict "ask", 3 allow rules, and 1 deny rule
    When the summary is printed
    Then the Policy block shows the file path
    And the default verdict
    And a one-line rule summary: "3 allow, 1 deny, anything else asks"
    And the provenance line: "source: found in this directory"

  Scenario: Auto-created policy is called out in the source line
    Given no `lns-policy.yaml` exists in the working directory
    When the run starts
    Then the Policy block source line reads "auto-created (no policy in this directory)"
    And the default verdict is "ask"

  Scenario: Explicit --policy is called out in the source line
    Given the command is `lns run --policy ~/team/strict.yaml ubuntu`
    When the run starts
    Then the Policy block source line reads "source: --policy ~/team/strict.yaml"

  Scenario: Phase lines fill in fields the summary left as placeholders (cold cache)
    Given the image is not in the local cache
    When the service resolves the image
    Then `✓ resolved ubuntu:latest @ sha256:…` is printed
    When layer pull completes
    Then `✓ pulled N layers   (Xs · YMB)` is printed
    When the microVM boots
    Then `✓ booted microVM   (Xs)` is printed
    When the session is ready
    Then `✓ session ready` is printed
    And finally `✓ started run 2a2a2a2a2a2a` is printed before the attached session takes over

  Scenario: Layer downloads show a live in-place progress bar on a terminal
    Given the image is not in the local cache
    When the service streams pull progress halfway through the download
    Then the terminal shows an in-place pull progress bar at 50%
    And the pulled completion line erases the bar and starts at column 0

  Scenario: Warm image cache collapses the resolve+pull phases
    Given the image is already cached locally
    When the run starts
    Then a single `✓ image cached` line is printed in place of resolve+pull
    And the boot and session-ready phase lines still follow

  Scenario: A failing phase surfaces with the same cadence
    Given the image `ubuntu` cannot be resolved
    When resolution fails
    Then a line `✗ resolve failed: <reason>` is printed in place of the resolve `✓`
    And the already-printed summary block is not redrawn or erased
    And the process exits non-zero with the same reason

  Scenario: Non-TTY output stays log-safe
    Given stdout/stderr is not a TTY
    When the run starts
    Then the summary block and phase lines are emitted
    And no spinner, cursor movement, or ANSI escape sequences are emitted
    And each phase line is a single complete line

  Scenario: Detached run (-d) prints the summary, phase lines, then run id
    Given the command is `lns run -d ubuntu`
    When the run starts
    Then the summary block is printed
    And the phase lines stream as usual
    Then `run 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a` is printed on its own line (the existing scripting contract)
    And the process exits 0 without attaching

  Scenario: The pre-start phase stream is distinct from in-run logs
    Given a run is starting
    Then phase lines (`✓ …`, `✗ …`) lead up to `✓ started run 2a2a2a2a2a2a`
    And `RunLog` frames from the workload are rendered exactly as they are today
    And the attached session takes over the terminal cleanly with no leftover phase output

  Scenario: Every run-status line appears exactly once in check form
    Given the image is not in the local cache
    When the cold-cache run plays through resolve, boot, session, and finish
    Then each run-status phase appears exactly once
    And no right-aligned developer-format line reaches the user
    And no raw enum verb like `SessionReady` appears verbatim
    And `Started  run #N` and `Finished  in …` never appear right-aligned
    And the final byte of the run output is a newline

  Scenario: Workload output without a trailing newline still leaves the prompt on a fresh line
    Given the image is not in the local cache
    When the workload prints without a trailing newline and then exits
    Then the final byte emitted to the user's terminal is a newline

  Scenario: Redirected stdout receives the workload's bytes unchanged
    Given the image is not in the local cache
    And the user's stdout is redirected to a pipe or file
    When the workload prints without a trailing newline and then exits
    Then the captured stdout is exactly the workload's bytes with no appended newline
