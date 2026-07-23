# Changelog

## [0.16.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.15.0...lns-v0.16.0) (2026-07-23)


### ⚠ BREAKING CHANGES

* the connector CLI command, the `connectors:` policy/ manifest key, the `~/.lns-connectors.yaml` catalog path, and the `LNS_CONNECTORS_PATH` env var replace their `integration`-named predecessors. Rename an existing `~/.lns-integrations.yaml` by hand and update any `integrations:` keys in tracked `lns-policy.yaml` files.

### Features

* add `lns uninstall` to stop sandboxes and remove the install ([a6d0ed7](https://github.com/lensapp/lens-sandbox/commit/a6d0ed7aa14b5104504c7b4b278954879f7c3fe0))
* add inline fileset authoring schema ([30591fe](https://github.com/lensapp/lens-sandbox/commit/30591fe72de7b49b4b3f3bf522ad90de9580ca61))
* **artifact:** introduce lns-artifact, the kind: Sandbox schema ([85c1a55](https://github.com/lensapp/lens-sandbox/commit/85c1a557b62652b817145c60eebc207a826c1102))
* **cli:** collapse the surface to one noun, the sandbox ([1fb0711](https://github.com/lensapp/lens-sandbox/commit/1fb0711709a5c75f2bebb31baf0f6b5e27cf3949))
* confirm a pulled sandbox's host binds and filesets before the run starts ([bc8e19e](https://github.com/lensapp/lens-sandbox/commit/bc8e19e95efda94fb43688be1dc863bef932afb6))
* disclose and confirm a pulled sandbox's named volumes alongside binds and filesets ([21a3e18](https://github.com/lensapp/lens-sandbox/commit/21a3e18eac6e6ff869e52d7ef80a37d9ff561b1c))
* disclose and publish inline filesets ([afb15ad](https://github.com/lensapp/lens-sandbox/commit/afb15ad54d556adc05a8c003a325fe49cdc52a38))
* **filesets:** cap inline filesets at 1 MiB total and 256 files ([45a44dd](https://github.com/lensapp/lens-sandbox/commit/45a44ddbe47d57ed324157360d9287ae76989dce))
* inspect a definition file by yaml path or -f/--file, offline ([773752e](https://github.com/lensapp/lens-sandbox/commit/773752eeb5e0065f4797879d8a38a27a7f12b820))
* **ipc,policy,ocsf:** shared groundwork for the sandbox surface ([ca90bde](https://github.com/lensapp/lens-sandbox/commit/ca90bdec27199255952d759d548301ddcf6857a7))
* materialize inline filesets ([0d42db6](https://github.com/lensapp/lens-sandbox/commit/0d42db66be410573c31806a3504d40090201127e))
* point the credential card at the token-mint command ([3ad7996](https://github.com/lensapp/lens-sandbox/commit/3ad7996d6260c223552a53859d592ee174ad4986))
* **policy:** bundle the claude-code-subscription credential integration ([b3cadf6](https://github.com/lensapp/lens-sandbox/commit/b3cadf646f49a8621e4adf111492ffda2982ad42))
* publish a definition file selected with -f/--file ([271d1b1](https://github.com/lensapp/lens-sandbox/commit/271d1b17c2155b6ed14c5954c6256fdfcc15cab7))
* run a definition file named by a path-shaped yaml reference ([d7d4b5e](https://github.com/lensapp/lens-sandbox/commit/d7d4b5e9d9bd2fed26234d6e195253df533a527b))
* **sandbox:** filesets transfer to the workload user by default ([c2ab6c0](https://github.com/lensapp/lens-sandbox/commit/c2ab6c0270fe06c9252e6a7e3fe953255ac723f2))
* select the run definition with -f/--file, exclusive with REF ([7fec5ca](https://github.com/lensapp/lens-sandbox/commit/7fec5ca04a4313e20d1e9f3988cbac569f6cccee))
* **service:** ingest, plan, and run sandbox artifacts ([d90f55f](https://github.com/lensapp/lens-sandbox/commit/d90f55f1f01f32adb24b820b465edacd48fbad22))
* show env in lns inspect for a cached sandbox reference ([e717d30](https://github.com/lensapp/lens-sandbox/commit/e717d30d07221a6c1e01b0ead6b47bac7ca6cb46))
* show env in lns inspect for a local definition ([fa455e0](https://github.com/lensapp/lens-sandbox/commit/fa455e039002bfd425dc773264cd7fce34d58e39))
* validate a definition file selected with -f/--file ([ac1d6c2](https://github.com/lensapp/lens-sandbox/commit/ac1d6c2eaea5ce49a357401d92eba7da9ae1b122))


### Bug Fixes

* accept keyed --mount syntax for run.volume defaults ([7ba1017](https://github.com/lensapp/lens-sandbox/commit/7ba1017601673330332532e8398e7fcd97b62333))
* **artifact:** enforce registry artifact boundaries ([ae2223d](https://github.com/lensapp/lens-sandbox/commit/ae2223d5a5f2e2c6f9d346fce5f181993c9216cd))
* **artifact:** reject unknown definition fields ([9f20df5](https://github.com/lensapp/lens-sandbox/commit/9f20df55754d814c00d04f56f2c6e0a36b14f1bf))
* auto-remove --rm runs on workload exit, not CLI stream drain ([af45339](https://github.com/lensapp/lens-sandbox/commit/af453390bf2bc587b171cb4e9b6b91a303d0a581))
* **build:** unlink bin/ artifacts before copying so macOS's cached vnode signature doesn't SIGKILL the next exec ([0758179](https://github.com/lensapp/lens-sandbox/commit/07581795a4687e837ea36c2d6dc2efa949d32ed1))
* cap buffered blob pulls and fail a run on an index-write error ([c081dd9](https://github.com/lensapp/lens-sandbox/commit/c081dd98247eb29e1ebfc3f678aa96a24f78e659))
* cap path string lenth ([9bf2987](https://github.com/lensapp/lens-sandbox/commit/9bf298771596d701e5f3ab53357527be19d22e21))
* clamp a pulled artifact's allow rules under a user's deny-by-default policy ([66dab12](https://github.com/lensapp/lens-sandbox/commit/66dab12ec39330966d31dca2f56412771b88da02))
* confirm only pulled sandbox mounts ([a8cc806](https://github.com/lensapp/lens-sandbox/commit/a8cc8060b18d5b1967235a4ebb629a201e6586f5))
* **credentials:** seed workload env only for declared integrations ([78a3301](https://github.com/lensapp/lens-sandbox/commit/78a330146aa16c146b48364f0890f59539087fa6))
* **credentials:** suppress a connectable whose domain an applied integration owns ([9e72450](https://github.com/lensapp/lens-sandbox/commit/9e724508174ad20467a3bb25efb083b99f2e3e3d))
* **dashboard:** stop text corrupting after long uptime by replaying texture deltas eframe drops on hidden-window passes ([3489ef8](https://github.com/lensapp/lens-sandbox/commit/3489ef870ee20ab6a50947a55bbfedbe3c24a7f1))
* derive default transport variant ([adfa949](https://github.com/lensapp/lens-sandbox/commit/adfa94963047c66de615ee7e758482b352fd4234))
* disclose only the artifact-declared mounts that survive a -v override ([7a29a85](https://github.com/lensapp/lens-sandbox/commit/7a29a85b312247028190ccb83e77aae88a30c055))
* distinguish a push connectivity failure from an auth refusal ([111ab5b](https://github.com/lensapp/lens-sandbox/commit/111ab5bf8295bd86f9870006a9ea36679aa6260a))
* drop unused --pull flag, symmetric exec command parsing, document compat flags ([f1f4624](https://github.com/lensapp/lens-sandbox/commit/f1f4624d5dd3d4de4909d264edfbdb96c4da0cdd))
* fail a sandbox run when its base-image retention record can't be written ([3fca4f6](https://github.com/lensapp/lens-sandbox/commit/3fca4f63cdc5fa3e45f5d71b268156a32ec21c5d))
* format path-length bail and pin the inline path cap with a test ([50521c6](https://github.com/lensapp/lens-sandbox/commit/50521c670080b2a0556380d2b401b2d1d3c9d816))
* gate published filesets with the strict digest-pin validator ([e501d2b](https://github.com/lensapp/lens-sandbox/commit/e501d2b573da918f80149d96def06d27a28a51ef))
* harden fileset artifact boundaries ([1898bfd](https://github.com/lensapp/lens-sandbox/commit/1898bfd798d17db7dfc5b60f176a7b1ea5da1efb))
* harden sandbox artifact inspection ([f1602c3](https://github.com/lensapp/lens-sandbox/commit/f1602c30a92b1b951d9277ca3f2a68de8cc2ee95))
* **init:** seed the sandbox user ahead of an image alias for the same uid ([d5c2268](https://github.com/lensapp/lens-sandbox/commit/d5c22686c0f6184c080b30d0c8e4834f4503c2d2))
* keep artifact-declared credentials within the user's network boundary ([4ae62bc](https://github.com/lensapp/lens-sandbox/commit/4ae62bc32f9b587aedff9aa181a6dbe0353e0759))
* keep image ENTRYPOINT when a command overrides CMD ([67db1b2](https://github.com/lensapp/lens-sandbox/commit/67db1b2e503a7fc7b20edac0250750b10f3afc8f))
* keep workload command verbatim and address run-flag review feedback ([09eea8b](https://github.com/lensapp/lens-sandbox/commit/09eea8b7da84ad0e29f7c978c5b332b579196ff3))
* match a by-reference sandbox run to its digest-keyed retention record ([c525364](https://github.com/lensapp/lens-sandbox/commit/c52536495dddaff54e5ca341d450aab2d3869fc3))
* name the selected definition in the run banner and drop the lns-init hint for missing variants ([a71b99f](https://github.com/lensapp/lens-sandbox/commit/a71b99fa0ee0a3da9a9590b1e9c075c18180ffff))
* never registry-qualify a path-shaped lns run target ([c14483e](https://github.com/lensapp/lens-sandbox/commit/c14483e60de8824cc0a7cb5913e661c5f4567a7d))
* offer artifact-declared integrations reactively instead of arming them ([219ac29](https://github.com/lensapp/lens-sandbox/commit/219ac29c797dd3465db911ccf83cecae1f499a53))
* only fall back to the cached artifact when a run is genuinely unknown ([ce19162](https://github.com/lensapp/lens-sandbox/commit/ce19162b4b7700d8363223b307bc9d694ce57f2d))
* preserve fileset file modes through push and pull ([a8c3060](https://github.com/lensapp/lens-sandbox/commit/a8c30608ef2965e5dc2a204a56ac6294730446a1))
* preserve sandbox policy and cache boundaries ([c81eeac](https://github.com/lensapp/lens-sandbox/commit/c81eeac5e31288dc0a175cb5c0fd05a42897188d))
* record a run's base image so rm/prune can't delete it ([f3265c1](https://github.com/lensapp/lens-sandbox/commit/f3265c100cc122b54dfdfc355072f48d5d78e5c7))
* refuse a runtime -v mount that shadows the /.lens namespace ([06022ea](https://github.com/lensapp/lens-sandbox/commit/06022ea6add7c0de519ec191ef39496f4cd07178))
* refuse a volume that mounts over the /.lens runtime namespace ([8d93b7e](https://github.com/lensapp/lens-sandbox/commit/8d93b7e0318a5a8009f50e000f31db875b14687b))
* reject a malformed fileset digest at push time ([22fea5e](https://github.com/lensapp/lens-sandbox/commit/22fea5ea83217bdde6d34802d4b791f8dcd418d4))
* reject a non-positive declared layer size before fetching layers ([64f30fe](https://github.com/lensapp/lens-sandbox/commit/64f30febb5a2b8a39a6e16b82b675a04d0225a01))
* reject control-char fileset paths and strip setuid on fileset pull ([a49ada6](https://github.com/lensapp/lens-sandbox/commit/a49ada6b9268c225bf30aa2f0ee7646ecfe77597))
* reject malformed env keys and non-positive resource requests ([abd0495](https://github.com/lensapp/lens-sandbox/commit/abd0495e61f1c2977e90da78621786dd6caa89ff))
* remove upstream transport from local policy surface ([cd06580](https://github.com/lensapp/lens-sandbox/commit/cd06580878a056efa714573d1ab3e9138401b448))
* report effective sandbox launch configuration ([c9abbe9](https://github.com/lensapp/lens-sandbox/commit/c9abbe912b5fb9d45aca19d8599f97281a922e31))
* resolve CI failures ([c45d833](https://github.com/lensapp/lens-sandbox/commit/c45d833814d4ec4651bf6872776a1852b99c47a6))
* restore missing tempdir setup in policy unit test ([ef53665](https://github.com/lensapp/lens-sandbox/commit/ef5366579330ea5afa804a16f391b3b0f729604a))
* route a push-scope refusal at upload through the sign-in recipe ([736f34b](https://github.com/lensapp/lens-sandbox/commit/736f34b9e7551dce6dc5bed16b78e86bfae8614c))
* skip a value-consuming global's value in sandbox-nested normalization ([6c83517](https://github.com/lensapp/lens-sandbox/commit/6c835172367db2195ba9ad9b56f2f8d1e8de0d6d))
* suppress a connectable that collides on an applied injection domain ([af8f23a](https://github.com/lensapp/lens-sandbox/commit/af8f23a8f4bc9e19bb05e412146e50bfdb55faa3))
* suppress connectables that overlap an applied integration's domain by wildcard or case ([1287fec](https://github.com/lensapp/lens-sandbox/commit/1287fec8ddbafeea646910e43691f7fc27ebb983))
* treat "no active run with id" as a run miss too in rm/inspect fallthrough ([52ce598](https://github.com/lensapp/lens-sandbox/commit/52ce5982effa936b4a19cb1ea70c1f56829c6db2))
* truncate a pulled fileset's digest by chars so a crafted ref can't panic the run summary ([eb34988](https://github.com/lensapp/lens-sandbox/commit/eb3498839c1725ac8f25e86a9317311a08dc2d30))
* validate a credential slot's env-var name like spec.env keys ([0b45a5a](https://github.com/lensapp/lens-sandbox/commit/0b45a5a55fe0cfbeb9129cb55e93cd976c6bc8c4))
* widen the shipped-policy warning to whole-TLD wildcards and moderate CIDRs ([b238d5c](https://github.com/lensapp/lens-sandbox/commit/b238d5cdcd082b82f17019c365daad51a2734abb))


### Code Refactoring

* rename the "integration" concept to "connector" ([4ec00f2](https://github.com/lensapp/lens-sandbox/commit/4ec00f2136de9af0ee338aa420f9f89f0ab35d6e))

## [0.15.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.14.0...lns-v0.15.0) (2026-07-02)


### ⚠ BREAKING CHANGES

* **lns-cli:** unify lns audit into one cross-sandbox OCSF timeline

### Features

* **cli:** add `list` alias for `ls` and `sandbox ls` ([3db4880](https://github.com/lensapp/lens-sandbox/commit/3db488066c2e4f328437eef08cc57960d89a7d41))
* **dashboard:** record the run image in a launch event and show it per sandbox ([cc83d07](https://github.com/lensapp/lens-sandbox/commit/cc83d0719521d87a524591f658d7678b86ac5e59))
* **lns-cli:** unify lns audit into one cross-sandbox OCSF timeline ([0f148b2](https://github.com/lensapp/lens-sandbox/commit/0f148b2e3d850c2ceaebe2db92d6cc7eef0afb90))
* **lns-ipc:** opaque hex run ids and connection-ledger wire types ([55b4391](https://github.com/lensapp/lens-sandbox/commit/55b43911b2a1f36fb37c10615f9bf0f4a83c6ad2))
* **lns-service:** OCSF audit recording, durable ledger, egui dashboard, client-attributed egress ([3611c16](https://github.com/lensapp/lens-sandbox/commit/3611c16b48cc9dbbff5965ec5cdc331c4519e241))
* make the `lns run` detach chord a docker-style detach ([0f63739](https://github.com/lensapp/lens-sandbox/commit/0f63739c0d9296731f388923489b15d74f941177)), closes [#58](https://github.com/lensapp/lens-sandbox/issues/58)
* **ocsf:** add the lns-ocsf crate — strict OCSF v1.7.0 event builders ([c1e5e55](https://github.com/lensapp/lens-sandbox/commit/c1e5e55f718bdda116353f5ad496057b1a72a42a))


### Bug Fixes

* **audit:** degrade the ledger per-line and harden name resolution + calendar math ([b45db4e](https://github.com/lensapp/lens-sandbox/commit/b45db4eb25c28464f501b730fee96be6f249ba1d))
* **audit:** harden the OCSF trail against unreadable and forged rows ([3a29298](https://github.com/lensapp/lens-sandbox/commit/3a292981637700c8961fb25d335c05bf29840b80))
* **audit:** reject out-of-range timestamps and pin core by full SHA ([cc40f57](https://github.com/lensapp/lens-sandbox/commit/cc40f57d4a1ea560deab83e55ee9620247e9faee))
* **dashboard:** read a finished sandbox's auto-name from the audit trail ([d5e54a5](https://github.com/lensapp/lens-sandbox/commit/d5e54a55990db78f18877375c2fec0e1e8518d62))
* **lns-service:** report accurate RunDetach errors, not always "no active run" ([d6d715f](https://github.com/lensapp/lens-sandbox/commit/d6d715fe0be8eebdedc53182c079017235954123))

## [0.14.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.13.0...lns-v0.14.0) (2026-06-25)


### Features

* **lns-cli:** add -q/--quiet to lns run and exec ([c1b1828](https://github.com/lensapp/lens-sandbox/commit/c1b182806c51a0bb930f0f987444f64f6d2bd908)), closes [#101](https://github.com/lensapp/lens-sandbox/issues/101)


### Bug Fixes

* **cli:** make -i/-t negatable on run and exec ([20cf5e4](https://github.com/lensapp/lens-sandbox/commit/20cf5e445bf177242c3479804bcd137d2ccd5413)), closes [#94](https://github.com/lensapp/lens-sandbox/issues/94)
* **lns-service:** clear the approval card's drop shadow when it closes ([e806007](https://github.com/lensapp/lens-sandbox/commit/e80600746fe43513b3d1cf33e1e068e6bb072640))
* **lns-service:** disclose env var + destination domain on credential approval card (M10) ([0a82e8e](https://github.com/lensapp/lens-sandbox/commit/0a82e8e25def3fbb338cc42a6eb9b45364895079))
* **lns-service:** update approval_preview example for new disclosure fields (M10) ([0c19072](https://github.com/lensapp/lens-sandbox/commit/0c190721032c318d2d98f3334acaf7f0fe580899))

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
