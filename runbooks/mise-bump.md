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

   **What the signature does and does not cover.** The minisign check covers the
   engine binary's sha256 — the bytes that reach a guest. It does **not** cover
   the source tarball the snapshot is generated from: upstream publishes no sum
   for it, so that download is trusted on your review in step 2, not on a
   signature. The snapshot is both the provisionability allowlist and the source
   of every `source_host` audit label, so read its diff as security-relevant.

2. Review the snapshot diff: renamed tools or backend changes surface here, and
   this review is the only control on the tarball the snapshot came from.
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
