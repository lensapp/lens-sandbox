@microvm
Feature: a fileset writes into a subpath the bind excludes
  A bind shares a host directory, and one subpath of it is excluded, so the guest
  gets a mask there instead of the host contents. A fileset writes into that mask.
  The workload reads the file, rewrites it in place, and replaces it by rename —
  none of which reaches the host. The bind is mounted entry by entry to make that
  possible, so a new top-level entry the workload creates stays in the guest while
  an existing host file it edits still writes through. These outcomes are guest-
  observable and need a booted microVM, so they are @microvm and run only via
  `make e2e-microvm`. The commands use /bin/sh builtins atop the standard alpine
  base.

  Scenario: the workload rewrites a seeded file inside the mask and the host never sees it
    Given the LNS service is running
    And a host directory with a file "greeting" containing "from-host"
    When the user runs a microVM command "/bin/sh -c 'read v < /work/seeded/state.json; echo seeded=$v; echo second > /work/seeded/state.json; read v < /work/seeded/state.json; echo rewritten=$v; echo third > /work/seeded/pending; /.lens/guest-tools/bin/busybox mv /work/seeded/pending /work/seeded/state.json; read v < /work/seeded/state.json; echo renamed=$v; read s < /work/greeting; echo sibling=$s; echo edited > /work/greeting; echo newfile > /work/created'" from a sandbox binding that directory and seeding "seeded"
    Then the exit code is 0
    And the output contains "seeded=from-the-document"
    And the output contains "rewritten=second"
    And the output contains "renamed=third"
    And the output contains "sibling=from-host"
    And the host bind directory has a file "greeting" containing "edited"
    And the host bind directory has no entry "seeded"
    And the host bind directory has no entry "created"
