# Changelog

## [0.7.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.6.0...lns-v0.7.0) (2026-06-05)


### Features

* auto-start the service on install and login (lns service enable/disable) ([7196a95](https://github.com/lensapp/lens-sandbox/commit/7196a95a9f7a6126aa756907299299d504aba996))


### Bug Fixes

* gate run-scoped local-render suppression on a shared tty ([9376cea](https://github.com/lensapp/lens-sandbox/commit/9376ceac42d3fe9205a09ecb7161939df7d263e3))
* never spawn a competing service instance during enable ([0a9758f](https://github.com/lensapp/lens-sandbox/commit/0a9758f89b94527f925d4cf69c9ba3078c5294d9))
* only newline-pad attached stdout when it is a terminal ([00a5887](https://github.com/lensapp/lens-sandbox/commit/00a58870523914a0764c625354b319b9bcf13f3e))
* render lns run status lines once in ✓ form ([0219d76](https://github.com/lensapp/lens-sandbox/commit/0219d76a298dc22b515bacc7302a502e924efb10)), closes [#3](https://github.com/lensapp/lens-sandbox/issues/3)

## [0.6.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.5.0...lns-v0.6.0) (2026-06-03)


### Features

* **update-check:** add anonymous update-and-security check ([04347b1](https://github.com/lensapp/lens-sandbox/commit/04347b19e669663f7b8a640fa52b9cf06fe22fb0))


### Bug Fixes

* **update-check:** timeout fetch; drop in-app disclosure ([97a2efb](https://github.com/lensapp/lens-sandbox/commit/97a2efbf469a9c746f2faa9a5e4ced513de4d106))

## [0.5.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.4.0...lns-v0.5.0) (2026-06-03)


### Features

* **lns-cli:** accept credential values from stdin via --value-stdin ([f438efe](https://github.com/lensapp/lens-sandbox/commit/f438efefcc8cc67094ac7a9f705ac6e06fb92a09))
* **lns-cli:** add lns credential add/add-injection/remove for custom providers ([980ff9b](https://github.com/lensapp/lens-sandbox/commit/980ff9bd89522c329ecf67071451b2b25e9ab34f))
* **lns-cli:** add lns credential set/clear/list commands ([87889e7](https://github.com/lensapp/lens-sandbox/commit/87889e74108bad22a504bd1859c58b9c263fd178))
* **lns-cli:** add lns policy allow/deny/list/remove commands ([ae394be](https://github.com/lensapp/lens-sandbox/commit/ae394be44288b12fca70c258fa187c51100615f6))
* **lns-cli:** publish guest ports with docker -p grammar ([0ad5b6b](https://github.com/lensapp/lens-sandbox/commit/0ad5b6b36000486b2238ccaff21ecd23ad8cca80))
* **lns-run:** add -e/--env KEY=VALUE for non-secret workload config ([d205c23](https://github.com/lensapp/lens-sandbox/commit/d205c2351064124da5832412eaea3b193ce14cdc))
* **volumes:** persistent named volumes for `lns run` ([5678edb](https://github.com/lensapp/lens-sandbox/commit/5678edb6f2f171a95436f57a4375811bde7a50a3))


### Bug Fixes

* address adversarial-review findings on the credential CLI ([72884e5](https://github.com/lensapp/lens-sandbox/commit/72884e5cd2acf942bb044e274ca136c216daa272))
* **credentials:** build token/basic-auth/x-api-key injections after the lns-policy move ([c65ab84](https://github.com/lensapp/lens-sandbox/commit/c65ab84cda29d876ad95089d549b7fac55a4b1e7))
* **lns-cli:** warn that a new credential injection needs a sandbox relaunch ([79841a5](https://github.com/lensapp/lens-sandbox/commit/79841a5f1fd60203ee35481edafa5364d7539909))

## [0.4.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.3.0...lns-v0.4.0) (2026-05-28)


### Features

* **lns-cli:** add `lns update` self-updater ([bcfe4a4](https://github.com/lensapp/lens-sandbox/commit/bcfe4a49a7b8b1c671bb2512b4423dec1a6a1df5))
* **lns-cli:** add `lns update` self-updater ([dd02d53](https://github.com/lensapp/lens-sandbox/commit/dd02d533ec4c8431c3a28863c09b9e794c274d69))
* **lns-cli:** auto-create lns-policy.yaml when --policy is unset ([0268b2c](https://github.com/lensapp/lens-sandbox/commit/0268b2cb17723056dcbe47373a489e8bf9313655))
* **lns-cli:** print up-front run summary before any service round-trip ([adfd89f](https://github.com/lensapp/lens-sandbox/commit/adfd89f71f7b64c5e79ae5512b82041b0e6aaf2e))
* **lns-cli:** render pre-start phase lines and move Started to SessionReady ([7190ee8](https://github.com/lensapp/lens-sandbox/commit/7190ee8a46befdc58ff990e5a9003247631db939))
* **lns-cli:** wire failing-phase + non-TTY + pre/post-distinction scenarios ([aa27280](https://github.com/lensapp/lens-sandbox/commit/aa272809954d4cf3af1e34a070b3dbbdc2570dfb))


### Bug Fixes

* **lns-cli,bump-kernel:** restore --help via #[arg(help=)] attribute strings ([8d63214](https://github.com/lensapp/lens-sandbox/commit/8d6321497b24c089cd1ae779f03d0fef5d723295))
* **lns-cli:** apply clippy bool-assert-comparison fixes ([1bbfd0b](https://github.com/lensapp/lens-sandbox/commit/1bbfd0bb467632c0fc9da95add5fb16b89a3fdca))
* **lns-cli:** best-effort service relaunch on partial update failure ([cb9f8d2](https://github.com/lensapp/lens-sandbox/commit/cb9f8d2fbe56ca4e06df25bc1cd7469661a28307))
* **lns-cli:** hoist as_secs() out of log::error! arg list ([7e82e75](https://github.com/lensapp/lens-sandbox/commit/7e82e757be2c01a43777ccbfe0e2fbd217f85c0c))
* **lns-cli:** make `lns update` commit recoverable, not just staging ([7b2a4c8](https://github.com/lensapp/lens-sandbox/commit/7b2a4c8a663766b3fbd54755ead2d248d004e2f2))
* **lns-cli:** map env::consts::OS to uname-style sysname in uname fallback ([6ecb818](https://github.com/lensapp/lens-sandbox/commit/6ecb818e17b763c526b6f1feccc354539493bb9b))
* **lns-cli:** reject non-regular tar entries in release-tarball extract ([a7efc6e](https://github.com/lensapp/lens-sandbox/commit/a7efc6ea2eedd334fffc762361bfb0f119ef0b2f))
* **tests:** cucumber binaries must exit non-zero on scenario failure ([7c5a512](https://github.com/lensapp/lens-sandbox/commit/7c5a512773aa1498ccd526bff893aec72016e364))

## [0.3.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.2.0...lns-v0.3.0) (2026-05-21)


### ⚠ BREAKING CHANGES

* **lns-cli:** single logging API — log::{error,warn,info,debug}!
* **lns-cli:** replace -q/-v with --log-level <error|warn|info|debug>

### Features

* **coverage:** instrument with cargo-llvm-cov and gate per-file at 100% ([881a721](https://github.com/lensapp/lens-sandbox/commit/881a721311d7045b6a755f27b5c5d0e75e29cfa4))
* Docker-style interactive shell + lns run -d / exec / kill / ls on lns-service ([41232e4](https://github.com/lensapp/lens-sandbox/commit/41232e412133cac62356c6058aa23f9e40e884ad))
* integrate session broker with lns-service architecture ([ac635e5](https://github.com/lensapp/lens-sandbox/commit/ac635e57fa2760fa80097c2861bd143733cac8d1))
* **lns-2:** tray-resident background service with lns service start|stop|status ([364810b](https://github.com/lensapp/lens-sandbox/commit/364810be37660f7cbb408d5e0b5e5dd444df2f4c))
* **lns-cli:** add 'lns service start|stop|status' thin-client routing ([da0d862](https://github.com/lensapp/lens-sandbox/commit/da0d862647cbbfbb5cba796370680a24fee42abf))
* **lns-cli:** instrument lns.run as tracing root span ([8c381c8](https://github.com/lensapp/lens-sandbox/commit/8c381c82808128334fe4532d9060d9ba04184663))
* **lns-cli:** require running service for lns run ([be023fa](https://github.com/lensapp/lens-sandbox/commit/be023fa0021fc6a63f13d084cf1a3208e02d24fe))
* **lns-cli:** silent default output; show progress chrome with -v ([7341e0e](https://github.com/lensapp/lens-sandbox/commit/7341e0e9b1c7d1592e2ee70d28bf50a54607560f))
* **lns-service,lns-cli:** migrate stdout/stderr streaming to WireFrame ([d88e591](https://github.com/lensapp/lens-sandbox/commit/d88e591a60b8a3fdb80515c0b3451a27ccdf2377))
* **lns-service:** add LNS_SESSION_BROKER_BIN env override for dev loop ([791ef9f](https://github.com/lensapp/lens-sandbox/commit/791ef9fb1f05628a25e5f5435b209a06135af1ba))
* port lns run -d, exec, kill, ls onto lns-service architecture ([10d4226](https://github.com/lensapp/lens-sandbox/commit/10d42263f45dfe761aeddee76df01c07e84e2a06))


### Bug Fixes

* address self-review findings for the CLI/daemon split ([f34daba](https://github.com/lensapp/lens-sandbox/commit/f34dabae50d823c99fc945bb087b5c7308455a12))
* **ci:** cross-link lns-init musl target via rust-lld ([33c8d71](https://github.com/lensapp/lens-sandbox/commit/33c8d71e6870def217a4cf25c4335720fdc786e4))
* **ci:** cross-link lns-init musl target via rust-lld ([49a07af](https://github.com/lensapp/lens-sandbox/commit/49a07af6fbfb7483fa13cd8fceea2e4f52c86860))
* **lns-cli:** don't early-exit run loop on stdin EOF (piped run regression) ([22a3cf4](https://github.com/lensapp/lens-sandbox/commit/22a3cf4c19036fc166a6aeae1f41774ba1ee0b10))
* **lns-cli:** make service client honest, async, and daemon-safe ([c6acfca](https://github.com/lensapp/lens-sandbox/commit/c6acfca6dbd049e450edb748a7893a2a465e13f2))
* **lns-cli:** reset SIGPIPE to SIG_DFL on startup ([d244949](https://github.com/lensapp/lens-sandbox/commit/d244949daacad56506ab9b23354e12a939dab16e))
* **lns-cli:** resolve workspace target dir via cargo metadata ([d2c5a16](https://github.com/lensapp/lens-sandbox/commit/d2c5a16f40eee57ae67ef4f80755a96b6668a6b4))
* **lns-ipc:** make default_socket_path() fallible ([0027015](https://github.com/lensapp/lens-sandbox/commit/0027015a89d763bb0338e718dff3ed868c870457))
* **lns-service:** address self-review findings on PR [#12](https://github.com/lensapp/lens-sandbox/issues/12) ([d8ea74a](https://github.com/lensapp/lens-sandbox/commit/d8ea74aee4597656b00b9c63747a26d6fce420fd))
* **lns-service:** close SIGINT/CancelRun races on the streaming run path ([2ba5dd5](https://github.com/lensapp/lens-sandbox/commit/2ba5dd5f91b3780b1b1419eee61bc9ae81ffcb32))
* **lns-service:** key per-run paths by allocated run_id, not daemon PID ([ab921fb](https://github.com/lensapp/lens-sandbox/commit/ab921fbc95576d0ae1c34e6c073c1ced1685a4ec))
* **log:** preserve structured fields beyond verb/message ([ffe4a86](https://github.com/lensapp/lens-sandbox/commit/ffe4a867bae8c9d1c7de17d753427e47080e64ae))
* **mutation-test:** pin per-crate cargo-mutants output and install GTK deps ([48b042b](https://github.com/lensapp/lens-sandbox/commit/48b042bb21b7209c9a9cf443ef8e17a4703f0564))
* **post-rebase:** restore loop opener; migrate broker driver to WireFrame ([8c0e670](https://github.com/lensapp/lens-sandbox/commit/8c0e670a37a13b4896e08ff45fbebc1ec042d069))


### Code Refactoring

* **lns-cli:** replace -q/-v with --log-level &lt;error|warn|info|debug&gt; ([8c98c7a](https://github.com/lensapp/lens-sandbox/commit/8c98c7a1d59abf9ce9745829555773cb3008cf1f))
* **lns-cli:** single logging API — log::{error,warn,info,debug}! ([d270951](https://github.com/lensapp/lens-sandbox/commit/d27095195fb46b4a623c819305776f0d10bab040))

## [0.2.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.1.0...lns-v0.2.0) (2026-05-15)


### Features

* import lns-cli, lns-init, CI, scripts from nexus-monorepo[#246](https://github.com/lensapp/lens-sandbox/issues/246) ([2772a20](https://github.com/lensapp/lens-sandbox/commit/2772a20f9d5144161a7d5b0886ccd15b1e91e7a4))
* lns-cli and lns-init crates with build + release CI ([213318c](https://github.com/lensapp/lens-sandbox/commit/213318c31a7595aeebc50d60d566139a8056c84a))
* **lns:** derive kernel pin from kernels.toml at build time ([99dc1ad](https://github.com/lensapp/lens-sandbox/commit/99dc1ad8bad321d0ecf18ff51cd5efd417a154cb))


### Bug Fixes

* unblock make complexity on linux ci ([2f1fe2c](https://github.com/lensapp/lens-sandbox/commit/2f1fe2c73345678648c49a3e772c331355a12b92))
