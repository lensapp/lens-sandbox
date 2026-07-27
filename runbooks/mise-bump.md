# Bumping the mise engine pin

The tool-provisioning engine (mise), its provisioner rootfs images, and the
companion artifacts are pinned in `crates/lns-service/mise.toml`; the tool-name
registry snapshot lives at `crates/lns-service/src/tools/registry.snapshot`.
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

2. Review the snapshot diff: renamed tools or backend changes surface here.
   `crates/lns-service/src/tools/registry.rs` pins the spike-validated names in
   its unit tests, so a vanished mainline tool fails `make test`.

3. Companions (`[provisioner_rootfs]`, `[static_curl]`, `[[companion]]` apks)
   are bumped by hand when needed — re-download the artifact, re-hash it, and
   update the pin. Alpine apk pins go stale when the `v3.20` branch rotates a
   package; the failed sha256 check names the artifact.

4. Gate the bump PR on the real e2e scenarios — registry semantics can drift
   between mise versions:

   ```bash
   make e2e-microvm LNS_E2E_FEATURE=tools
   ```
