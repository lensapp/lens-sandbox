# Changelog

## [0.13.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.12.0...lns-v0.13.0) (2026-06-25)


### Features

* **build:** derive guest musl target from host CPU arch ([d5ddfe8](https://github.com/lensapp/lens-sandbox/commit/d5ddfe8c6ec34ec6812e890d4ed5727cf4f68934))
* **forward:** enable port publishing (-p) on Linux ([95c53e9](https://github.com/lensapp/lens-sandbox/commit/95c53e99b799fbd7e7e4c36fb7f9cdaf82cc28c7))
* **run:** unify the run orchestrator across Vz and Cloud Hypervisor ([ae7f9ff](https://github.com/lensapp/lens-sandbox/commit/ae7f9ff13737a20a58cbd6c78f6c8acdd0406fe6))
* **service:** add a reusable ui component system (theme, button, card) ([c5287a1](https://github.com/lensapp/lens-sandbox/commit/c5287a1d929ec59968c4d0cf345fc19d8b3fd87f))
* **service:** category icons, badges, and tonal buttons for approval cards ([7837e3f](https://github.com/lensapp/lens-sandbox/commit/7837e3fdf508dba3b5f710804d2defebd8519041))
* **service:** fold stacked approvals into a macOS-style notification pile ([dde2cb7](https://github.com/lensapp/lens-sandbox/commit/dde2cb70c33225dc63c35df03781b9abbfcf098b))
* **service:** redesign approval cards as macOS-notification-style panels ([4757fa6](https://github.com/lensapp/lens-sandbox/commit/4757fa64b6532a9f917b623164c13da6e04926b1))
* **service:** render the approval window in the macOS system font ([09bbcba](https://github.com/lensapp/lens-sandbox/commit/09bbcba9830b174e7ac772b1c1795aaca9bba16b))
* **tray:** run the Linux tray on a dedicated GTK main loop ([562137d](https://github.com/lensapp/lens-sandbox/commit/562137df147d87e55d22b937401aa2a06e4ecfa2))
* **vm:** Cloud Hypervisor backend foundations + GuestTransport seam ([1d24773](https://github.com/lensapp/lens-sandbox/commit/1d24773727ada1269f62b6483a0473f9e12d61f5))
* **vm:** enforce read-only host binds at the host on Cloud Hypervisor ([2aa3650](https://github.com/lensapp/lens-sandbox/commit/2aa3650ff2be30f1a4343ce9e11f15a4ecd1f0f2))
* **vm:** resolve cloud-hypervisor/virtiofsd from PATH on Linux ([8dd4795](https://github.com/lensapp/lens-sandbox/commit/8dd479516c643700d96e4e4bda51fa98f17a2671))
* **vm:** serve host bind mounts as virtio-fs shares on Cloud Hypervisor ([7350ccb](https://github.com/lensapp/lens-sandbox/commit/7350ccb4324cf504adc8825124eec5ef314b6819))


### Bug Fixes

* **cloud-hypervisor:** add KVM preflight, surface console.log on failure, reap virtiofsd ([e5d9487](https://github.com/lensapp/lens-sandbox/commit/e5d9487dd98d8c0531fb5e675e579551e2cb0b71))
* **cloud-hypervisor:** address review — seek console tail, fix modprobe hint ([d47d277](https://github.com/lensapp/lens-sandbox/commit/d47d277058aa8df01acdcd095db3987ec7543310))
* **cloud-hypervisor:** apply || in the production KVM hint to match test ([f90fa33](https://github.com/lensapp/lens-sandbox/commit/f90fa33155f0ec36c04800ef3cca693e6256d2ab))
* **cloud-hypervisor:** reap virtiofsd when cloud-hypervisor fails to spawn ([5034fb0](https://github.com/lensapp/lens-sandbox/commit/5034fb09bb2d441a02f6c3f0fe2d616d95055c4f))
* **lns-service:** close the Linux build (de-gate test RunHandle, document unsafe) ([0d6b88c](https://github.com/lensapp/lens-sandbox/commit/0d6b88ce0635d5563877ec9568b4083e672e9898))
* **policy:** make add_rule idempotent so re-approvals don't duplicate routes ([8d3e000](https://github.com/lensapp/lens-sandbox/commit/8d3e0007e3515b7636c61a72861daf52d48c9b3f))
* **run:** surface the real boot error when launch drops the connector ([a606da1](https://github.com/lensapp/lens-sandbox/commit/a606da113b8e2bf961d05655f7d3fa25d8b5b8c2))
* **service:** gate quiet_debug_overlays behind debug_assertions ([47cc53a](https://github.com/lensapp/lens-sandbox/commit/47cc53adb579ffd19b650a11d734c516e42563d1))
* **tray:** recolor the macOS template icon to white on Linux ([73a4878](https://github.com/lensapp/lens-sandbox/commit/73a48787f5fe6b3da84bedd0db7631de4745ad4f))
* **vm:** also gid-map the bind so the workload can write under the namespace sandbox ([e07025b](https://github.com/lensapp/lens-sandbox/commit/e07025b40dd20dfef63898b5fe9e478cc5244c7c))
* **vm:** declare image_type=raw on Cloud Hypervisor disks ([5a834ac](https://github.com/lensapp/lens-sandbox/commit/5a834ac4be15bbf6e47c72513e62e5059dc4e8a0))
* **vm:** integrate the host-bind field into the Cloud Hypervisor path ([c4a8736](https://github.com/lensapp/lens-sandbox/commit/c4a8736eaa6377c716da973151f157ebf878508d))
* **vm:** make host binds reachable by the workload on Cloud Hypervisor ([9809029](https://github.com/lensapp/lens-sandbox/commit/980902948ad3419dd74e8dea5b8480e4542855c1))
* **vm:** map the workload uid to the host user for binds (virtiofsd 1.10.0) ([f0502ae](https://github.com/lensapp/lens-sandbox/commit/f0502ae5fc5a3f47ec08d72805b04268c161cbc8))
* **vm:** resolve virtiofsd from /usr/libexec so no env var is needed ([f1b99df](https://github.com/lensapp/lens-sandbox/commit/f1b99df99d8ff8ad637d1591044fbf86eaad9665))
* **vm:** run bind virtiofsd under the namespace sandbox so --uid-map applies ([5b9d8fe](https://github.com/lensapp/lens-sandbox/commit/5b9d8fe8ac91d92abc20481c9e4a18c79c224ace))

## [0.12.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.11.0...lns-v0.12.0) (2026-06-17)


### Features

* **lns-cli:** disambiguate -v host binds from named volumes ([2f742e6](https://github.com/lensapp/lens-sandbox/commit/2f742e67a576b0434cba0e5285b6f26cfb9336fb))
* **lns-cli:** scan host binds for secrets and resolve KEEP/DROP before run ([f49b77b](https://github.com/lensapp/lens-sandbox/commit/f49b77b4ac8c45460b1ceaf08723fb4d31933328))
* **lns-cli:** show host-bind secret disposition in the run summary ([8057b2b](https://github.com/lensapp/lens-sandbox/commit/8057b2b5a5d353ee3f3dc8b2b98f4ad0ee87cc16))
* **lns-cli:** widen the host-bind secret heuristic to more credential shapes ([78750c4](https://github.com/lensapp/lens-sandbox/commit/78750c48704807f3dd4ed199296b1ff26853dc89))
* **lns-policy,lns-cli:** secret detection, .lensignore, KEEP/DROP decision store ([5408928](https://github.com/lensapp/lens-sandbox/commit/5408928467b14f82c661dda0efb8a8d6634a0eff))


### Bug Fixes

* **lns-cli:** drop any .lensignore-listed file from a host bind, not only secret-shaped ones ([49c843e](https://github.com/lensapp/lens-sandbox/commit/49c843e475819657751d6fe23bb4577778df5203))
* **lns-cli:** honor nested .lensignore entries and refuse bind-escaping ones ([5fb96e0](https://github.com/lensapp/lens-sandbox/commit/5fb96e078e03215173ebbe889ba10724180c5446))
* **lns-cli:** refuse to drop a secret whose filename contains whitespace ([a958a07](https://github.com/lensapp/lens-sandbox/commit/a958a07b024829bc3adfbfab28113f053fc416c6))
* **lns-cli:** reject quotes and control chars in dropped host-bind paths, not only whitespace ([3199a8f](https://github.com/lensapp/lens-sandbox/commit/3199a8fbd1a002ad76da50aad2956b6cebaa4f77))

## [0.11.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.10.0...lns-v0.11.0) (2026-06-16)


### Features

* add lns login for OCI registries ([4c2c387](https://github.com/lensapp/lens-sandbox/commit/4c2c387d1675b19a1ef3518fe45ffe7bf656bf19))
* address runs by name across the lifecycle ([40a32ab](https://github.com/lensapp/lens-sandbox/commit/40a32ab558e69c87e4424cfb7110fb7948509e96))
* **integrations:** add an OAuth 2.0 PKCE sign-in flow ([3d39867](https://github.com/lensapp/lens-sandbox/commit/3d398678b3960119159d4a0df9f40cf48eb8cf19))
* **integrations:** add Google as a device-flow oauth integration ([ad11029](https://github.com/lensapp/lens-sandbox/commit/ad1102907a60704046e07d15cd7a21bb08da69cd))
* **integrations:** ship OpenRouter as a bundled pkce integration ([e49a805](https://github.com/lensapp/lens-sandbox/commit/e49a805c1dea018bc649ea41dac3d1c9619c9c72))


### Bug Fixes

* cap HTTP download bodies before buffering them into memory ([adb7b08](https://github.com/lensapp/lens-sandbox/commit/adb7b0882befd6d9ed3a50d55fdd11334c95de23))
* **cli:** emit CRLF for attached run-log status lines in raw tty ([7f01bce](https://github.com/lensapp/lens-sandbox/commit/7f01bce45e2f25974a82cdd7e487b3d2eb2bd05b))
* **lns-cli:** don't hold std stdin/stdout locks across interactive run/exec ([8afcdb4](https://github.com/lensapp/lens-sandbox/commit/8afcdb4aeb0c91dbe1610e835c46346303148536))
* **lns-cli:** surface missing/corrupt audit anchor instead of silent clean verify ([5f02c2b](https://github.com/lensapp/lens-sandbox/commit/5f02c2b3b0bbe9386f54a71721706e73932e7b41))

## [0.10.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.9.0...lns-v0.10.0) (2026-06-12)


### Features

* **cli:** apply lns config defaults to lns run, per-run flags win ([cea6a37](https://github.com/lensapp/lens-sandbox/commit/cea6a37745ac0389b8f124cbf051128ca860bb69))
* **cli:** attach detaches without signalling the run ([e573e7e](https://github.com/lensapp/lens-sandbox/commit/e573e7e8b2d28ee4c24419146e5b4ae66203f522))
* **cli:** consolidate ls, exec, and kill under lns sandbox ([f13ccef](https://github.com/lensapp/lens-sandbox/commit/f13ccef654c9a8780aef6bf7a26dcc0ff86920e8))
* **cli:** lns config — get/set/unset/list persistent run defaults ([461af9a](https://github.com/lensapp/lens-sandbox/commit/461af9a35b31082e7c327dc5a6509729b89bfabf))
* **cli:** lns image subcommand family ([6b2c6c3](https://github.com/lensapp/lens-sandbox/commit/6b2c6c36b3b5c95aef8b2eb48e40560bfb253e8c))
* **cli:** lns sandbox — stop, logs, attach, inspect, stats ([a8c5271](https://github.com/lensapp/lens-sandbox/commit/a8c5271d2806d4312fc9ad8ae893c01b64335210))
* **cli:** lns sandbox rm and prune for finished runs ([fc8bbdb](https://github.com/lensapp/lens-sandbox/commit/fc8bbdb940b86ca68f39e3a21a01939a66cc010a))
* **cli:** lns volume subcommand family ([07e7d5c](https://github.com/lensapp/lens-sandbox/commit/07e7d5ce8578240af46efbd2fc690983d603aa77))
* **cli:** render a live progress bar and spinner during run pre-phase waits ([6025d98](https://github.com/lensapp/lens-sandbox/commit/6025d9886bb20dadc476051d4add574b995bd660))
* **config:** accept memory units and reject zero in run.cpus/run.mem defaults ([bf7c912](https://github.com/lensapp/lens-sandbox/commit/bf7c912b607b0f2757b5f224320b7ea1b835309e))
* **run:** add --env-file, -w/--workdir, and Docker-style memory units ([6652b1b](https://github.com/lensapp/lens-sandbox/commit/6652b1b6b14b756a8eea287ec319796dc4c3e6ab))
* **volumes:** collect prune failures instead of aborting ([3b88ca5](https://github.com/lensapp/lens-sandbox/commit/3b88ca5754afce49a87c956c442802ffd0d16f9e))
* **volumes:** report on-disk usage alongside logical size ([0829d2e](https://github.com/lensapp/lens-sandbox/commit/0829d2e6759e1df243890a64c31aab1d2a9951f5))

## [0.9.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.8.0...lns-v0.9.0) (2026-06-10)


### ⚠ BREAKING CHANGES

* **credentials:** make integrations the only credential-provider mechanism

### Features

* **cli:** add integration catalog management and connect/disconnect ([9baa466](https://github.com/lensapp/lens-sandbox/commit/9baa466d203ea9131a60fea14085f08540ea0998))
* **cli:** connect oauth integrations through an interactive device sign-in ([c7b970f](https://github.com/lensapp/lens-sandbox/commit/c7b970fc5301b1a014a52bbf219c8232745ea2c3))
* **policy:** add integration catalog and richer route rules ([3f25fc2](https://github.com/lensapp/lens-sandbox/commit/3f25fc2500dae1478aa3735de50bbaf45f298308))
* **policy:** add oauth AuthKind schema and bundled github_oauth integration ([02486b8](https://github.com/lensapp/lens-sandbox/commit/02486b82d083a6fbb6c593b55d60c819554765df))
* **service:** add host-side OAuth device-flow engine and an oauth token entry ([92c7f3c](https://github.com/lensapp/lens-sandbox/commit/92c7f3c2d44e35ff119b6172568dba4e35c48d48))
* **service:** connect to GitHub via a named consent card and browser sign-in ([bda876f](https://github.com/lensapp/lens-sandbox/commit/bda876f2e79f672ff36a6e4c66c1324c8fa1eca5))
* **service:** offer a token fallback when oauth connect is blocked ([a5ab665](https://github.com/lensapp/lens-sandbox/commit/a5ab6654ead59b382a234f46552d47c1dd0dcff1))


### Bug Fixes

* **cli:** accept connected integration ids in `lns credential set` ([cfa0dae](https://github.com/lensapp/lens-sandbox/commit/cfa0dae10e6a17b064e312fbed49583d6301b76b))


### Code Refactoring

* **credentials:** make integrations the only credential-provider mechanism ([bd8d9d0](https://github.com/lensapp/lens-sandbox/commit/bd8d9d0da741454ecae665b930439298411fb5bd))

## [0.8.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.7.0...lns-v0.8.0) (2026-06-08)


### Features

* **run:** default OCI image runs to the image's USER ([09aeadb](https://github.com/lensapp/lens-sandbox/commit/09aeadbecc7ef5c4576e908101a7b13d151ea5b9))


### Bug Fixes

* **run:** honor the group component of an image's OCI USER ([3a0b21e](https://github.com/lensapp/lens-sandbox/commit/3a0b21e284bbed3dfca692d3452ad7be628700cd))

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
