# setup-lns

Installs the [lns](https://github.com/lensapp/lens-sandbox) CLI on a GitHub
Actions runner.

The action reads this repository's releases, downloads the tarball for the
runner's platform, verifies the `.sha256` published beside it, extracts only
`lns` into `$RUNNER_TOOL_CACHE` and appends that directory to `GITHUB_PATH`. It
never runs the get.lns.run installer, and it never installs `lns-service` — a
runner has no microVM, so the commands that reach for the service (`lns run`,
`lns sandbox`) do not work there. The offline document verbs (`lns artifact
validate`, `lns artifact push`) do.

## Usage

```yaml
- uses: lensapp/lens-sandbox/actions/setup-lns@lns-actions-v0
- run: lns artifact validate -f lns.yaml
```

Pin a version instead of tracking the newest release:

```yaml
- uses: lensapp/lens-sandbox/actions/setup-lns@lns-actions-v0
  with:
    version: 0.25.0
```

An lns release does not open a pull request here. Bump `version:` in your own
repository when you want a newer CLI; omit it to follow the newest release.

## Inputs

| Input | Default | Description |
| --- | --- | --- |
| `version` | `latest` | Release to install, e.g. `0.25.0` (a leading `v` is accepted). `latest` resolves the newest published lns release. |
| `token` | `${{ github.token }}` | Token used to read the release and download its assets. |
| `binary` | — | Path to an already built `lns`. Nothing is downloaded; that binary is installed instead. For a workflow that tests its own build. |

## Outputs

| Output | Description |
| --- | --- |
| `version` | The lns version on `PATH` after the action ran. |

## Platforms

| Runner | Asset |
| --- | --- |
| Linux x86_64 | `lns-<version>-linux-x86_64.tar.gz` |
| Linux aarch64 | `lns-<version>-linux-aarch64.tar.gz` |
| macOS arm64 | `lns-<version>-darwin-aarch64.tar.gz` |

Any other platform fails the step: no lns release covers it. Checksums are
verified with `sha256sum` on Linux and `shasum -a 256` on macOS.
