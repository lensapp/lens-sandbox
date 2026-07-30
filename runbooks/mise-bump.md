# Bumping the mise engine pin

The tool-provisioning engine (mise), its provisioner rootfs images, and the
companion artifacts are pinned in `crates/lns-service/mise.toml`; the tool-name
registry snapshot lives at `crates/lns-artifact/src/tools/registry.snapshot`.
Users never see or choose the engine version, and the tool cache key excludes
it, so a bump never re-provisions installed tools.

## Steps

1. From the workspace root:

   ```bash
   cargo run -p bump-mise -- bump --version <NEW_VERSION>
   ```

   This verifies the release's `SHASUMS256.txt` against mise's published
   minisign key before any pin lands, rewrites `[engine]` in `mise.toml`, and
   regenerates `registry.snapshot` (names, aliases, and preferred download
   backends) from the release's `registry/` directory.

   It also refreshes `crates/lns-artifact/src/tools/index_snapshots/*.txt` —
   real version-index bodies for a shape-diverse tool set
   (`INDEX_SNAPSHOT_TOOLS` in `bump-mise/src/operations.rs`). These are the
   fixtures behind the resolver's reality contract test
   (`every_line_the_real_index_publishes_is_a_usable_stable_pin`): if upstream
   starts publishing a version shape our pinning rules mishandle, the refresh
   turns that test red at the bump instead of at a user's push. A red run here
   is the mechanism working — fix the rule, not the fixture.

   **What the signature does and does not cover.** The minisign check covers the
   engine binary's sha256 — the bytes that reach a guest. It does **not** cover
   the source tarball the snapshot is generated from: upstream publishes no sum
   for it, so that download is trusted on your review in step 2, not on a
   signature. The snapshot is both the provisionability allowlist and the source
   of every `source_host` audit label, so read its diff as security-relevant.

2. Review the snapshot diff: renamed tools or backend changes surface here, and
   this review is the only control on the tarball the snapshot came from.

   **Also re-check `core_source_host`** in
   `crates/lns-artifact/src/tools/registry.rs`. It is a hand-maintained table of
   where each `core:` tool's bytes come from, and it is emitted as `lns_source`
   in the audit chain — an attestation. Nothing links it to the engine's actual
   download hosts and it does not appear in the snapshot diff, so a release that
   moves one (say `java` off `api.adoptium.net`) would leave the chain asserting
   the old origin indefinitely. For each entry in that table, confirm the pinned
   engine still fetches that tool from that host; drop the entry rather than
   guess, since `None` means "not claimed" and is always safe.
   `crates/lns-artifact/src/tools/registry.rs` pins the spike-validated names in
   its unit tests, so a vanished mainline tool fails `make test`.

3. Companions (`[provisioner_rootfs]`, `[ca_bundle]`, `[static_curl]`,
   `[[companion]]` apks) are bumped by hand when needed — re-download the
   artifact, re-hash it, and update the pin.

   Alpine's mirror keeps only the current version of each package in a branch, so
   when `v3.20` rotates one of the `[[companion]]` apks the pinned filename stops
   existing: the fetch **404s before any hash is compared**, and the error names
   the URL. Re-pin to the version the branch now carries. `[ca_bundle]` is
   deliberately not an apk for this reason — curl.se keeps every dated snapshot,
   so that pin only moves when we choose to move it.

4. Gate the bump PR on the real e2e scenarios — registry semantics can drift
   between mise versions:

   ```bash
   make e2e-microvm LNS_E2E_FEATURE=tools
   ```
