:robot: I have created a release *beep* *boop*
---


<details><summary>lns: 0.18.0</summary>

## [0.18.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.17.0...lns-v0.18.0) (2026-08-14)


###   BREAKING CHANGES

* make a directory's decisions the mixin the specification says they are
* keep which connectors a project connected out of the file it commits
* a definition carries `name` at the top level, and a document that nests it under `metadata` is refused by name. A `labels` block is refused too.
* a definition names its egress under `spec.egress`. The old spelling is refused by name rather than accepted alongside it.
* let the later source decide where two disagree about a destination
* a definition must spell its kind in lower case. Pre-1.0, so there is no shim: the old spelling is gone rather than accepted too.
* a sandbox declares the credential it needs, never a connector
* **policy:** decide destinations with rules, not a default verdict
* **policy:** adopt egress.http as the canonical route table
* **cli:** lns volume inspect renames its keys, size_bytes to sizeBytes, disk_bytes to diskBytes and in_use_by to inUseBy. A script reading the old names gets null rather than an error, so update it before upgrading.

### Features

* a sandbox declares the credential it needs, never a connector ([3527be3](https://github.com/lensapp/lens-sandbox/commit/3527be34f67f38f6badff1ed3c0420a5047f2233))
* **a11y:** label the approval window's dismiss controls ([dbd6fa0](https://github.com/lensapp/lens-sandbox/commit/dbd6fa0fce0e7205a109c2f1704aa94edcaac102))
* add bump-mise operator tooling and move the Claude Code example onto spec.tools ([a9e33be](https://github.com/lensapp/lens-sandbox/commit/a9e33bedddbb62b346327a74996f82345efa80c6))
* attribute every entry of a resolved sandbox to the source that decided it ([c456539](https://github.com/lensapp/lens-sandbox/commit/c456539588f014f8e70973af4b97ee366c7b73ca))
* cache provisioned tool trees and record resolved versions per machine ([e2cffe5](https://github.com/lensapp/lens-sandbox/commit/e2cffe5e222570fafa0ab3b0ef24c120ba51baaa))
* carry a document's name above its spec ([42a93af](https://github.com/lensapp/lens-sandbox/commit/42a93af73bb085de463494d18186e5b92bc6da4e))
* **cli:** give lns audit --format table|jsonl ([5ca1f7a](https://github.com/lensapp/lens-sandbox/commit/5ca1f7aba34db85e91e1a09e0167afc67acacdd4))
* **cli:** give lns config list and get --format json ([02ccb74](https://github.com/lensapp/lens-sandbox/commit/02ccb7487e0a85ede1517a1745735cb7450a437e))
* **cli:** give lns connector list and grants --format json ([4b8d5b1](https://github.com/lensapp/lens-sandbox/commit/4b8d5b1b729e1a7421ce7fb9d0efb2fec28da569))
* **cli:** give lns policy list --format json ([a8923d2](https://github.com/lensapp/lens-sandbox/commit/a8923d2b41c0782342192707032687503866e60b))
* **cli:** give lns ps --format json ([feff7fa](https://github.com/lensapp/lens-sandbox/commit/feff7fa94831bb258afa099cd623db0807ccf5f1))
* **cli:** give lns sandbox ls --format json ([4075a6e](https://github.com/lensapp/lens-sandbox/commit/4075a6e7e7efecdcf0ebd0cf709c8033653d5e2b))
* **cli:** give lns service status --format json ([55d8bbf](https://github.com/lensapp/lens-sandbox/commit/55d8bbf2265f6997bc4ea01ca1457cef0bcebfeb))
* **cli:** give lns volume ls --format json ([c7db0b7](https://github.com/lensapp/lens-sandbox/commit/c7db0b72f2317a2aa95e0e9cea38d05bf096b8e8))
* **cli:** inspect, revoke, and disconnect-clear per-workload grants ([a66f01d](https://github.com/lensapp/lens-sandbox/commit/a66f01d0e90e7fd7acfdebbd7478155c6ad997d8))
* **cli:** shared machine-readable output seam ([e7bb085](https://github.com/lensapp/lens-sandbox/commit/e7bb085d5104cae1c2a1e867367394dd4ad92228))
* **cli:** tell connect when this project holds a standing decline ([6b527d3](https://github.com/lensapp/lens-sandbox/commit/6b527d33f3c49fc99e2c54fa3b1f8aebc6f9e43a))
* compose declared tools from cache, record, and provisioner with first-resolution pinning ([7c0acfc](https://github.com/lensapp/lens-sandbox/commit/7c0acfca0eb403095c7a92fc704e9de190c3ea3a))
* declare developer tools via spec.tools with offline shape validation ([4ad4964](https://github.com/lensapp/lens-sandbox/commit/4ad4964d0d8b7e250c512871804d48ee9c0b029c))
* detect the workload image's libc flavor from its layer tars ([364e00e](https://github.com/lensapp/lens-sandbox/commit/364e00e4f7f2cfb1aef9e15ce35bfdba94b175be))
* disclose declared tools in inspect and the run summary ([d4eb56f](https://github.com/lensapp/lens-sandbox/commit/d4eb56fbad4528531473843454e5bd8cb6c3a2bb))
* keep which connectors a project connected out of the file it commits ([2d7e808](https://github.com/lensapp/lens-sandbox/commit/2d7e808f569799cf35d121c4c6e98bea8f1c44ab))
* key a connector grant on the mixins a run is composed of ([101474f](https://github.com/lensapp/lens-sandbox/commit/101474f71ff1d39305d8bbe4522a85d33144809c))
* let a definition declare a host file or a home-anchored bind ([b42fa20](https://github.com/lensapp/lens-sandbox/commit/b42fa208e431017906fcee7fa8ceb4bbb9ac98eb))
* let a definition declare the user it needs to run as ([b19b61a](https://github.com/lensapp/lens-sandbox/commit/b19b61a7a5b502bdb4928e837efdee54a3b9700a))
* let a definition exclude subpaths from a bind ([ac0e42f](https://github.com/lensapp/lens-sandbox/commit/ac0e42f50cae781bfbff4184885da9f3526c8368))
* let a definition size itself as a share of the host ([6e5226f](https://github.com/lensapp/lens-sandbox/commit/6e5226fa71f3866aba912a613bd4657465be8deb))
* let a document declare the mixins it layers on ([b74b449](https://github.com/lensapp/lens-sandbox/commit/b74b4490f671e0179ccffcd5b9b8b1c8c2592e96))
* let a document read a mixin from a directory beside it ([50ddd83](https://github.com/lensapp/lens-sandbox/commit/50ddd83e3a0dc5eb36c19d7ab224073ec5b972c4))
* let a published mixin be pulled and inspected ([e75ae41](https://github.com/lensapp/lens-sandbox/commit/e75ae414318da7fd011a36a63cf78291426e337d))
* let a resolution carry the directory's own decisions as its last source ([22cbc13](https://github.com/lensapp/lens-sandbox/commit/22cbc139e107b2ed025a80a772bb90d6000cd91d))
* let a user merge their own mixins into a run ([f0b802b](https://github.com/lensapp/lens-sandbox/commit/f0b802bd6e37bde82c506502de7bc70f544e1844))
* let the later source decide where two disagree about a destination ([7a85d9a](https://github.com/lensapp/lens-sandbox/commit/7a85d9a2b4eb380b2b35efc42d327b627948c488))
* make a directory's decisions the mixin the specification says they are ([e8d1450](https://github.com/lensapp/lens-sandbox/commit/e8d14508b5d5855c3bdbabc227d3089dd21a8640))
* name a document's egress where the specification names it ([3378778](https://github.com/lensapp/lens-sandbox/commit/33787787948b65a01964029d09912d03d7508190))
* pin resolved tool versions into the published artifact at push ([3a4489e](https://github.com/lensapp/lens-sandbox/commit/3a4489ef3be53ff3cb974940f77ae24cd0e89a2d))
* pin the mise engine, provisioner rootfs images, and companion artifacts ([8067146](https://github.com/lensapp/lens-sandbox/commit/80671467fefbf6262744e2db8a79c7ed6310a36c))
* **policy:** add per-workload connector grant store ([f852407](https://github.com/lensapp/lens-sandbox/commit/f85240795a91493e7ddaa868cef4e007b8dd76bc))
* **policy:** decide destinations with rules, not a default verdict ([e98b997](https://github.com/lensapp/lens-sandbox/commit/e98b9973d313bfeeacc9019af49253228a4ea662))
* **policy:** per-binary scoping for network routes ([ae9d21b](https://github.com/lensapp/lens-sandbox/commit/ae9d21b7300ca822ce9044a8949b4092ae2748eb))
* **policy:** raw TCP egress with treatment-aware approvals ([d6c3951](https://github.com/lensapp/lens-sandbox/commit/d6c3951301d02ec929a01d6aa1b7ee6e6da392c3))
* pre-provision a pulled sandbox's pinned tools so it starts offline ([6ae5e5f](https://github.com/lensapp/lens-sandbox/commit/6ae5e5fc397e4d33cfc01f83cf69498a8d6bb16e))
* provision declared tools in a disposable engine guest with a writable staging share ([6321296](https://github.com/lensapp/lens-sandbox/commit/6321296d3941c83dfb3cb0df066bd743474e15e6))
* provision declared tools pre-boot and prepend their bin paths to the workload PATH ([b58a110](https://github.com/lensapp/lens-sandbox/commit/b58a11057d67919f6658889d1c7253e2cc4d58cd))
* publish an already-exact tool pin without the index ([69565de](https://github.com/lensapp/lens-sandbox/commit/69565def75b78e771cbcabea3a962ac45437e0a4))
* re-resolve [@latest](https://github.com/latest) tools against the index on every run ([280dc49](https://github.com/lensapp/lens-sandbox/commit/280dc493a12b3a80d317d23896711e73118778c9))
* record a pull's tool acquisition on the machine audit chain ([e7046aa](https://github.com/lensapp/lens-sandbox/commit/e7046aa01421aa6f7685ffd037ba77211cdfa5f6))
* record tool provisioning in the run's audit chain ([5db2a73](https://github.com/lensapp/lens-sandbox/commit/5db2a7357ce1a5810f35194825ad1f4fc9b6318e))
* refuse unknown and plugin-backed tools against the pinned registry snapshot ([18ddc23](https://github.com/lensapp/lens-sandbox/commit/18ddc2387ee9900d58b7d2e0163846c4c03606d5))
* refuse unprovisionable tools at authoring time ([ae81794](https://github.com/lensapp/lens-sandbox/commit/ae817947d6a4b3830b4874a44ed4de17824eca8a))
* report the tool versions a push pinned ([d689f0e](https://github.com/lensapp/lens-sandbox/commit/d689f0e1f03b8b31307d93cc9b951d5b1c347661))
* resolve a published sandbox's mixins before it boots ([562b1e2](https://github.com/lensapp/lens-sandbox/commit/562b1e2a47350641878414bef31ca5aa1b6952f9))
* resolve a sandbox and its mixins into one merged document ([d3edf33](https://github.com/lensapp/lens-sandbox/commit/d3edf33eb1687a9907f41ba5acbce85cca95c6e1))
* resolve every run's decisions as the directory's own mixin ([6b44283](https://github.com/lensapp/lens-sandbox/commit/6b44283286ebbb5f002ffaea8e0c1402852a9369))
* reuse a tool tree only where its guest-mates are trusted ([5197c70](https://github.com/lensapp/lens-sandbox/commit/5197c70dd51d20abad4c0cc6a493288f9adda1d6))
* **service:** gate connector arming on per-workload grants ([4012673](https://github.com/lensapp/lens-sandbox/commit/4012673b37edb008a0f1c7d0cfcd23750aa5d025))
* **service:** grant a value already bound on this machine from the card ([d804661](https://github.com/lensapp/lens-sandbox/commit/d804661a3f6982c15db37c1ec39f569293e1bd6d))
* **service:** offer a reconnect alongside spending a bound connection ([4e3f31a](https://github.com/lensapp/lens-sandbox/commit/4e3f31a4a05a43a51893bcd0c593e8f9bae02aa9))
* **service:** record per-workload connector grants at consent ([45b0405](https://github.com/lensapp/lens-sandbox/commit/45b0405d84d09b2155329f9bcb0bdfe24b054995))
* **service:** remember a declined connector as a per-workload deny ([a3a5772](https://github.com/lensapp/lens-sandbox/commit/a3a57727ab089ca776dfc915dcf34cf416d871f3))
* spell a document's kind the way the specification writes it ([69e2270](https://github.com/lensapp/lens-sandbox/commit/69e2270d01c2b172cdc8a32159e226b0edde3bd9))
* validate and disclose declared tools in the offline author verbs ([05d2907](https://github.com/lensapp/lens-sandbox/commit/05d29077d18c532326bf3fd8c9bc2f56f2afcedd))
* warn at push when the index does not list an exact tool pin ([f7ef447](https://github.com/lensapp/lens-sandbox/commit/f7ef447234d5a4c565bc1351fd1e227e028188aa))


### Bug Fixes

* a closed network card reads as undecided on the wire ([3943ac3](https://github.com/lensapp/lens-sandbox/commit/3943ac3b0753e813a703a2d626d27cd4aa5f4a60))
* allowlist an index-resolved version before it becomes a path ([00c0b66](https://github.com/lensapp/lens-sandbox/commit/00c0b66d5f67c45bbf699a71ffe73440b55952f1))
* allowlist declared tool versions so none can reach the driver shell ([ef9a933](https://github.com/lensapp/lens-sandbox/commit/ef9a9331b5a42da07714637a5e4a53c2670a096c))
* **artifact:** refuse two credential slots naming one connector ([fcd8a29](https://github.com/lensapp/lens-sandbox/commit/fcd8a29abd4532dd2fa204a012c9b223341867e4))
* bound staged tool tar ingestion ([8325545](https://github.com/lensapp/lens-sandbox/commit/8325545a4d2863738685d71ab1bf237cd4897023))
* bound the provisioner's stderr instead of failing on it ([30d9101](https://github.com/lensapp/lens-sandbox/commit/30d910171f39e9ead3f02499738abf4d2b8cb4ac))
* bound the version-index query so the fallback fires ([691da53](https://github.com/lensapp/lens-sandbox/commit/691da533f494adddc141637a61bfb0b1b0a886bc))
* bound the workload trust-store fetch ([42f7248](https://github.com/lensapp/lens-sandbox/commit/42f72485cce3ce7b879f00aec852218bb8c8db4b))
* **broker:** carry confine through the linux-only session fixture ([ec0b819](https://github.com/lensapp/lens-sandbox/commit/ec0b819b6261e5b2263ea957c8af0ea0c4f3f748))
* **broker:** confine an exec session to the workload's identity ([be05478](https://github.com/lensapp/lens-sandbox/commit/be054784b5f7cff22f190f52d41659642c996f9c))
* **broker:** keep the relay credentials out of a confined session ([7d3cbd4](https://github.com/lensapp/lens-sandbox/commit/7d3cbd4e13f6361502f05928bffc445a04052799))
* cache virtiofsd capability checks ([a3acefd](https://github.com/lensapp/lens-sandbox/commit/a3acefd3a2e434edf3e46f851a979d00f9fbcbb5))
* carry the version allowlist in the type that reaches the sinks ([4d9b8c9](https://github.com/lensapp/lens-sandbox/commit/4d9b8c94793bb6f4d48a8d473ee2f141cd081ad2))
* claim a tool's source host only when the backend names one ([7785bc6](https://github.com/lensapp/lens-sandbox/commit/7785bc6a24d35cf144b7944ce3b68193796a4abb))
* **cli:** clear per-workload grants before dropping a disconnected connector ([06b62d3](https://github.com/lensapp/lens-sandbox/commit/06b62d3bc3cafb657cf844fd200647c327ee953e))
* **cli:** default a bare registry reference to the Lens hub ([6aa8261](https://github.com/lensapp/lens-sandbox/commit/6aa82613e98d3af75c831b1cac91410f6ea81c22))
* **cli:** make ps report the same status shape as inspect ([8715bb6](https://github.com/lensapp/lens-sandbox/commit/8715bb6cccfecf31fb7af00c9551c667d79cfd56))
* closing an approval card no longer decides anything ([c3b0702](https://github.com/lensapp/lens-sandbox/commit/c3b0702b1b11857effbc8fac3432ba710963780f))
* disclose each tool as it commits, not after the set survives ([ee5c473](https://github.com/lensapp/lens-sandbox/commit/ee5c4731b211f30bc4d3049b1a010152fa882935))
* disclose the rules a run enforces when it layered on nothing ([2411b84](https://github.com/lensapp/lens-sandbox/commit/2411b846a23873d2f938192f6708fdfc2d403d5c))
* distinguish co-installed musl loaders ([d2a5844](https://github.com/lensapp/lens-sandbox/commit/d2a5844eff2e99d67292f24926d7912afab35a8c))
* exempt the volume-release syscall leaf from the coverage floor ([e3c1c96](https://github.com/lensapp/lens-sandbox/commit/e3c1c9642ab043ea517cd734991b9389920d3303))
* gate unprovisionable tools at authoring, not on consume ([458f0cb](https://github.com/lensapp/lens-sandbox/commit/458f0cbdcd8301d3b60f4178f445c59d1e22956e))
* give a workload the directories of its own tool trees ([3f3852a](https://github.com/lensapp/lens-sandbox/commit/3f3852aac85cc68c49ba6c3dadd89e2ef82ab530))
* give each tool its own provisioner engine state ([6392cbb](https://github.com/lensapp/lens-sandbox/commit/6392cbbe1303517d55f64be4165f6a2a48ac336f))
* give every workload guest a public trust store ([229b1e8](https://github.com/lensapp/lens-sandbox/commit/229b1e868c9e75d06a5960f13f1df74b1e27b678))
* give lns exec the run's own tool PATH ([b7d00f8](https://github.com/lensapp/lens-sandbox/commit/b7d00f8976b8ee06bb8d33353133c4c168976536))
* give the driver sole ownership of the marker channel ([4df06c4](https://github.com/lensapp/lens-sandbox/commit/4df06c42d83a40c88f7ccd9354330cab7dc59c6c))
* give volume images a journal so an unclean shutdown cannot corrupt them ([45059f8](https://github.com/lensapp/lens-sandbox/commit/45059f8307f8115aa006261ef4a88f7c3ef30ec7))
* honor OCI whiteouts in libc detection ([3121a9b](https://github.com/lensapp/lens-sandbox/commit/3121a9bb7ce759f6fd8ff625764e3425d117e2db))
* honor the record and manifest schema versions on load ([522348c](https://github.com/lensapp/lens-sandbox/commit/522348c8fde1089e1fd57ed0c5a5d8311537a4d1))
* inject only the companion libs a tool links ([0a5a04d](https://github.com/lensapp/lens-sandbox/commit/0a5a04da5254ddae3fb4e442d8e90505b01b4ce5))
* keep a blank PATH segment out of every workload, not just a tooled one ([368e07b](https://github.com/lensapp/lens-sandbox/commit/368e07b0f19bf126db3cda52428e321e2b2888a0))
* keep a warm tool set out of the install queue ([0838b9a](https://github.com/lensapp/lens-sandbox/commit/0838b9a7ff7c9ec48ea091b249a2efe57cd20e6a))
* keep blank segments off the workload PATH ([1c014aa](https://github.com/lensapp/lens-sandbox/commit/1c014aa7147da981c287f794dcb9d0027e19eedd))
* keep the bin-dir query's diagnostics for the failure cause ([62951d2](https://github.com/lensapp/lens-sandbox/commit/62951d27c56e59d8a0704f0aef25fb8fd0f40eec))
* keep the engine off the warm musl path ([dc1477b](https://github.com/lensapp/lens-sandbox/commit/dc1477b9e292097e9d75c9a657c63b208b299411))
* keep the guest stats probe's capture stdout-only ([8b0841f](https://github.com/lensapp/lens-sandbox/commit/8b0841fd41a023035153968a6b5f374413ce7b7f))
* keep the ps listing when one guest stops answering ([d007bea](https://github.com/lensapp/lens-sandbox/commit/d007beadd02a33e47360a598885004dc258eba93))
* keep the tool resolution record when prune reclaims the tool cache ([fd88e03](https://github.com/lensapp/lens-sandbox/commit/fd88e03fef9b4186ec8356f63784dba41ac739e4))
* let a ceiling permit what its own patterns cover ([29aeeef](https://github.com/lensapp/lens-sandbox/commit/29aeeef5cf469230d0873f2362907c1d2430b722))
* let a connector's own claim decide what it supplies ([d0b1848](https://github.com/lensapp/lens-sandbox/commit/d0b18485ea33d6310e63b1133a214a87c3196d29))
* let a definition's HOME survive the run-as user ([0c6b286](https://github.com/lensapp/lens-sandbox/commit/0c6b2868fc9245a14e02619d307ac517ad5d81bd))
* let a pull survive a failed tool pre-provision ([80c30f8](https://github.com/lensapp/lens-sandbox/commit/80c30f825ff3f8823b5e92effd9239ae80710f0e))
* let a tool's own name win the registry snapshot row ([3de5d73](https://github.com/lensapp/lens-sandbox/commit/3de5d733c522373d80ea5c36f5eaf5a5e387f6af))
* let a vendor-prefixed version resolve at push ([e3cff2a](https://github.com/lensapp/lens-sandbox/commit/e3cff2a7d050a11c9b4e9713093bae4a8685b2ea))
* let an exec session join the run it names ([07e2116](https://github.com/lensapp/lens-sandbox/commit/07e2116a034f843406fdee8d8c5508e828559941))
* let every prompting sandbox verb read the terminal it was invoked from ([06663cd](https://github.com/lensapp/lens-sandbox/commit/06663cd0300b2cc3d45b1a8e7de02fa23eb87c4c))
* let no later layer overturn a musl loader ([e686b73](https://github.com/lensapp/lens-sandbox/commit/e686b73d551ad4d81c6f5ad2a10d5d7f529ea78f))
* let the flag read the memory size a definition writes ([58cf8cd](https://github.com/lensapp/lens-sandbox/commit/58cf8cdc941bf721b78ce3743be7e4d5d2ac4d4c))
* let validate answer for the document every verb runs ([67ae060](https://github.com/lensapp/lens-sandbox/commit/67ae0608791a2ec6a2d6fc19d2aa30ef53ac9ce7))
* make an exact tool request cache-addressable without the record ([af0e77e](https://github.com/lensapp/lens-sandbox/commit/af0e77e7e516f92c82c401b77186dbe301765762))
* make guest content shares read-only ([f735aaa](https://github.com/lensapp/lens-sandbox/commit/f735aaa8b17b57f567f4124a9f17948b3be7c301))
* make pull provisioning retry warning neutral ([b5434a2](https://github.com/lensapp/lens-sandbox/commit/b5434a2e8ac126bbcf487553cc29e6d4ae6d10ae))
* make the provisioner guest boot and install against a live guest ([b7961ba](https://github.com/lensapp/lens-sandbox/commit/b7961bafdc9cbcc7182ee5d07b73acfd58899c07))
* make the snapshot's backend the one mise actually installs from ([ff56ce5](https://github.com/lensapp/lens-sandbox/commit/ff56ce5e893f6ffa2ac7ef8c88a509d2b7cd07c9))
* make the tool record durable and its neighbours serialized ([aef4586](https://github.com/lensapp/lens-sandbox/commit/aef4586a1f89b333df1e6be69728491e24b555ee))
* make tool provisioning a filterable, honestly-typed audit event ([960a686](https://github.com/lensapp/lens-sandbox/commit/960a686b6c36dc8261a2b7e23d66a91c14dda0d9))
* map every writable bind, including the root provisioner's ([9fa21bf](https://github.com/lensapp/lens-sandbox/commit/9fa21bf91766054eecb4acc9bf741b22c81be522))
* measure a tool symlink against the path injection will create ([3b477fb](https://github.com/lensapp/lens-sandbox/commit/3b477fbd45808a6a3a2920d08c4961546525f1a7))
* move tool tar ingest off the async worker ([0099fa1](https://github.com/lensapp/lens-sandbox/commit/0099fa1769a1c56ef99a6b5f4b837e5fae59a392))
* parse the provisioner's markers from stdout alone ([8083b7a](https://github.com/lensapp/lens-sandbox/commit/8083b7a04e35f12f7073496e0490a8fe9212bebb))
* pin the CA store to a snapshot upstream keeps ([2db2c31](https://github.com/lensapp/lens-sandbox/commit/2db2c3178d246eb61db10e9b2e53d3546b8e284d))
* place a declared tool's bin dirs from the engine, not a guess ([95e9853](https://github.com/lensapp/lens-sandbox/commit/95e9853cd65c0f543b6644c062413488bf1b88fd))
* **policy:** bound the wait for a contended grant-sidecar lock ([c70fbee](https://github.com/lensapp/lens-sandbox/commit/c70fbee360ea8ee77134bb84d02a17e8605b5e99))
* **policy:** keep one catch-all rule, not one behind another ([ca602fd](https://github.com/lensapp/lens-sandbox/commit/ca602fd455376f8bbaca0bdef660458a5aaf84ab))
* **policy:** let a disconnect cancel a grant the run is still holding ([2afdf3c](https://github.com/lensapp/lens-sandbox/commit/2afdf3c29b5a8f9571ec7bf49094804c64cbdf93))
* **policy:** make the migration steps runnable and count raw rules at launch ([a343887](https://github.com/lensapp/lens-sandbox/commit/a34388735fce6f954eb7249f12eebd1b466c90a3))
* **policy:** serialize grant-sidecar writes across processes ([8ebd4e9](https://github.com/lensapp/lens-sandbox/commit/8ebd4e986b25ad45d3af38ce4123023d009fc340))
* prune provisioned tool cache safely ([cd8fa24](https://github.com/lensapp/lens-sandbox/commit/cd8fa24e042fe420b88fa432cbd29d73a71b88cd))
* publish a run's tool paths with the gate that exposes them ([ddda302](https://github.com/lensapp/lens-sandbox/commit/ddda302c9478b5f8a77a020d188dc44d5f696543))
* quote the red-first argument-hint so strict YAML parsers load the skill ([a081ee1](https://github.com/lensapp/lens-sandbox/commit/a081ee1ef1eae891a02ea8532119e1c96630f254))
* re-apply ingest's tar guards to a manifest read off disk ([72cf5cb](https://github.com/lensapp/lens-sandbox/commit/72cf5cbe26f158eba5df7dadead01bf19b7036f7))
* re-apply the tree guards to a manifest read off disk ([e1ab69f](https://github.com/lensapp/lens-sandbox/commit/e1ab69fd6cde7cf9cdcfdfe35aaedc281370bc20))
* reap a provisioner guest that never became reachable ([e5f6eef](https://github.com/lensapp/lens-sandbox/commit/e5f6eeffb1e14e10c71a91607f8185d7f669d001))
* recognize resolver-emitted vendor versions as exact pins ([8297b99](https://github.com/lensapp/lens-sandbox/commit/8297b99b24125420bd5a9305844e6d1fc7a51061))
* refuse a provisioner run that reports one tool twice ([7d43298](https://github.com/lensapp/lens-sandbox/commit/7d432982f6922c046ee58da84503b065f0f86489))
* refuse a resolution two sources publish one host port on ([1240fa2](https://github.com/lensapp/lens-sandbox/commit/1240fa21139763fed0a092595c0117730cdd4b26))
* refuse a run that would boot unsupervised ([fc2ee7a](https://github.com/lensapp/lens-sandbox/commit/fc2ee7a9de374e8a758cc45fec22a03a95abf7ff))
* refuse a single-file bind instead of booting a guest that dies ([96623b5](https://github.com/lensapp/lens-sandbox/commit/96623b5c199278abaa5352a0d4aaf36736879fc9))
* refuse a staged tool tar that is not the regular file we wrote ([454ba27](https://github.com/lensapp/lens-sandbox/commit/454ba27e5be55964c36e188cc37805d5a67d6463))
* refuse a tool entry that lives under a symlink ([4301a51](https://github.com/lensapp/lens-sandbox/commit/4301a519122f89610b3ae0c9606f454ea8dfb94b))
* refuse a tool symlink that points out of its own tree ([a8680e7](https://github.com/lensapp/lens-sandbox/commit/a8680e7bdc10d25515d176f79c6e6cbc838d594b))
* refuse a tool symlink whose double slash hides a level ([4ef312a](https://github.com/lensapp/lens-sandbox/commit/4ef312a58e82394161910c50261a1787c3afd433))
* refuse driver-reported tool locations that escape the cache tree ([2d663d0](https://github.com/lensapp/lens-sandbox/commit/2d663d0692efca44a87a5e7b21aeb4d2677bf0df))
* refuse two mappings on one host port before boot ([0f14181](https://github.com/lensapp/lens-sandbox/commit/0f141813cebae8184a7ff856adc62072b30312ff))
* **registry:** route every registry client through one loopback-aware transport ([653bc6d](https://github.com/lensapp/lens-sandbox/commit/653bc6d2308e2659e6ca148731a52861fc92d174))
* reject virtiofsd without readonly support ([37f37ce](https://github.com/lensapp/lens-sandbox/commit/37f37ce2143eb8e434459a45c4a2e67cd84853f4))
* release each volume before the guest powers off ([b31efae](https://github.com/lensapp/lens-sandbox/commit/b31efae0171b13ddd1dd99ad721682c7018408ae))
* release image cache lock before tool provisioning ([81ff895](https://github.com/lensapp/lens-sandbox/commit/81ff8957d952a09c0774eea1e5b34069dad89338))
* report a guest left with no trust roots ([bcc0482](https://github.com/lensapp/lens-sandbox/commit/bcc0482ed834e32689b295a4c1d0e2d0761c3c20))
* report the resources a run will boot with, not the flags ([5edb799](https://github.com/lensapp/lens-sandbox/commit/5edb799507a8f085818030f08981af7c397375e4))
* require consent for published tool installers ([902c26c](https://github.com/lensapp/lens-sandbox/commit/902c26c9796728481d5f559d989779b8671af9cc))
* resolve a hardlinked tool file to its ingested digest ([364a7b4](https://github.com/lensapp/lens-sandbox/commit/364a7b49c9af1b62b0d858f506d210ef23dde4d1))
* resolve the run before looking up its tool PATH for exec ([a4be69c](https://github.com/lensapp/lens-sandbox/commit/a4be69c2a3f7993511111b9dddd192f39d979b7b))
* restore the stock npm launcher in provisioned tool trees ([b5c7ef9](https://github.com/lensapp/lens-sandbox/commit/b5c7ef9eea661a66787e15e23909c13ccbfbee24))
* reuse co-installed latest tool trees ([88ca7f5](https://github.com/lensapp/lens-sandbox/commit/88ca7f536f94f814bf7007b2b13e79d216850446))
* roll back partial mise bumps ([78d769d](https://github.com/lensapp/lens-sandbox/commit/78d769db4c0b29d7c455615b8b632bb2571339d4))
* root a definition's mixins at a directory the caller named ([709c0b6](https://github.com/lensapp/lens-sandbox/commit/709c0b666532a91c7ed03fe25b56d21e8c67692a))
* **run:** confine the primary session when the run is unsupervised ([d4f586d](https://github.com/lensapp/lens-sandbox/commit/d4f586d95371a7a7236cec8b2bd3b42a72105a29))
* **service:** canonicalize a definition's workload identity directory ([75844d6](https://github.com/lensapp/lens-sandbox/commit/75844d697c1c476eaf565d95b12a83dc0cdd5d16))
* **service:** close four gaps in the per-workload grant lifecycle ([a277c1a](https://github.com/lensapp/lens-sandbox/commit/a277c1ad8c468377657c61edc3a128203b61f555))
* **service:** grant a required slot's boot sign-in to its workload ([c3e0a4d](https://github.com/lensapp/lens-sandbox/commit/c3e0a4da21467f43264f0cfae9169ee874c1e787))
* **service:** let a disconnect cancel a boot sign-in's grant too ([8c34e23](https://github.com/lensapp/lens-sandbox/commit/8c34e233f8ba39b82c161e4066d54effee31f41c))
* **service:** offer a bound value only when one is actually bound ([56f5977](https://github.com/lensapp/lens-sandbox/commit/56f5977cd6cc89524e744650b409c97bbc6b4a1c))
* **service:** pin a grant decision to the forget count its card was asked against ([8be2654](https://github.com/lensapp/lens-sandbox/commit/8be2654e4cf819a278daebb172cf5f00890a3ee9))
* **service:** point an unidentifiable run at the stale-service restart ([930e688](https://github.com/lensapp/lens-sandbox/commit/930e688dc5e251b573b83186ad3155aaa8512444))
* **service:** refuse a run that resolves no workload identity ([70edc30](https://github.com/lensapp/lens-sandbox/commit/70edc30037994dc31bc26fdb1857a7c6a969b8a0))
* **service:** revalidate deny grants and fix remember_grant ordering ([b050418](https://github.com/lensapp/lens-sandbox/commit/b0504188facd005f1f6cf414787d3221fcb799c7))
* size pruned caches from metadata ([136c5aa](https://github.com/lensapp/lens-sandbox/commit/136c5aa413bbaf82bc17e725c75aa6fc2a9f2023))
* skip the libc memo when the layer set has no digests ([99b5d4c](https://github.com/lensapp/lens-sandbox/commit/99b5d4cccd540f30d22d23b4687e7f5fe089e02a))
* stop a credential slot withdrawing a granted route ([7d97f3f](https://github.com/lensapp/lens-sandbox/commit/7d97f3f9ac40132fdeff08f2883fbd20d78ad89b))
* stop dispatch re-locking the stdin its caller already holds ([28ea217](https://github.com/lensapp/lens-sandbox/commit/28ea217642b6526cd46f5d317a412e691ce54bb3))
* stop guessing aqua download hosts ([06fd256](https://github.com/lensapp/lens-sandbox/commit/06fd2563067b1abbad86eb6a506fc4c2fa9b36a3))
* stop offering `lns exec` a session it cannot open ([bc4b7bf](https://github.com/lensapp/lens-sandbox/commit/bc4b7bf3747ef3bc121ecdd2e6e7176e7d5ccb8b))
* stop the provisioner guest before its trees are named final ([d9b65a2](https://github.com/lensapp/lens-sandbox/commit/d9b65a276f1301f0c083516bd573b6c214f31325))
* stop the push index read at the cap instead of after it ([25f6927](https://github.com/lensapp/lens-sandbox/commit/25f69276912335cbf67de724678131b0df938450))
* **supervisor:** drop capabilities when the run-as user is root ([f7ae228](https://github.com/lensapp/lens-sandbox/commit/f7ae228ddb30d3afb1777fe9f1a6899fe72d9d84))
* **supervisor:** keep the run-as identity when the setuid is dropped ([fd9c17a](https://github.com/lensapp/lens-sandbox/commit/fd9c17a9e98956e799ffe4b663438d118336fc55))
* surface pull-time tool warnings ([d6fa2b8](https://github.com/lensapp/lens-sandbox/commit/d6fa2b8914f1e952887cb784883097f7caf08122))
* take a tool's whole install, not the dir the engine names ([c0cba37](https://github.com/lensapp/lens-sandbox/commit/c0cba377f14ca24f7ae828ef4b0a5b70c6769a98))
* transfer a volume's root to the run-as user on attach ([3ec05bf](https://github.com/lensapp/lens-sandbox/commit/3ec05bfa4d4eaa43835d2e5e04395b5911e75cb9))
* treat a guest trust-store link with no target as no store ([6625776](https://github.com/lensapp/lens-sandbox/commit/662577617b5ffb6a47cb55136991dccccc8b1910))
* validate dry-run tools before push preview ([2228ea8](https://github.com/lensapp/lens-sandbox/commit/2228ea8c559ea11778951a43e9a3ae52e4940525))
* warn about a differing digest only when a version is fuzzy ([7bf9060](https://github.com/lensapp/lens-sandbox/commit/7bf906089e082d3715820fb0f9f4c9798ac687ca))
* write only the developer's own policy back ([b1a7ccd](https://github.com/lensapp/lens-sandbox/commit/b1a7ccd3b6538be1294fc1144a9b22fb0781a7af))
* write the mise bump's two files atomically ([7860987](https://github.com/lensapp/lens-sandbox/commit/786098710fdcf26ce46657c117dc24ee9596e443))


### Performance Improvements

* keep the image libc scan off the async worker ([918545a](https://github.com/lensapp/lens-sandbox/commit/918545ab40ed2d6b0a92435fff10255ddfbdf54a))
* keep warm musl runs out of install queue ([aa2581e](https://github.com/lensapp/lens-sandbox/commit/aa2581ea4619cce8f100c15b7191913605a9c43a))
* memoize the image's libc flavor by its layer digests ([e1d0903](https://github.com/lensapp/lens-sandbox/commit/e1d0903522b1367667a4b89d037d0078c2d99aa7))
* resolve latest tool pins concurrently ([a3482cf](https://github.com/lensapp/lens-sandbox/commit/a3482cfa9a1c515752f9fd5ef7a4ef32847154a0))
* reuse latest pins after install lock ([30f3b14](https://github.com/lensapp/lens-sandbox/commit/30f3b14ed37ca18ef35fe3dc274df991a181100d))
* scan the pulled image's layers for libc in place ([6133303](https://github.com/lensapp/lens-sandbox/commit/6133303c98d4a29bacfa9c12387fe0b4cce05898))
* warm the workload trust store during concurrent prepare ([4dfe36b](https://github.com/lensapp/lens-sandbox/commit/4dfe36b0ee01dcf465d4a68ae20607783a1b47de))


### Code Refactoring

* **policy:** adopt egress.http as the canonical route table ([f302a01](https://github.com/lensapp/lens-sandbox/commit/f302a011b3931f3078d023fd3b48a58784b98a60))
</details>

<details><summary>e2e-tests: 0.18.0</summary>

## [0.18.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.17.0...e2e-tests-v0.18.0) (2026-08-14)


###   BREAKING CHANGES

* make a directory's decisions the mixin the specification says they are
* a definition carries `name` at the top level, and a document that nests it under `metadata` is refused by name. A `labels` block is refused too.
* a definition names its egress under `spec.egress`. The old spelling is refused by name rather than accepted alongside it.
* a definition must spell its kind in lower case. Pre-1.0, so there is no shim: the old spelling is gone rather than accepted too.
* a sandbox declares the credential it needs, never a connector
* **policy:** decide destinations with rules, not a default verdict
* **policy:** adopt egress.http as the canonical route table
* **cli:** lns volume inspect renames its keys, size_bytes to sizeBytes, disk_bytes to diskBytes and in_use_by to inUseBy. A script reading the old names gets null rather than an error, so update it before upgrading.

### Features

* a sandbox declares the credential it needs, never a connector ([3527be3](https://github.com/lensapp/lens-sandbox/commit/3527be34f67f38f6badff1ed3c0420a5047f2233))
* add bump-mise operator tooling and move the Claude Code example onto spec.tools ([a9e33be](https://github.com/lensapp/lens-sandbox/commit/a9e33bedddbb62b346327a74996f82345efa80c6))
* carry a document's name above its spec ([42a93af](https://github.com/lensapp/lens-sandbox/commit/42a93af73bb085de463494d18186e5b92bc6da4e))
* **cli:** give lns volume ls --format json ([c7db0b7](https://github.com/lensapp/lens-sandbox/commit/c7db0b72f2317a2aa95e0e9cea38d05bf096b8e8))
* declare developer tools via spec.tools with offline shape validation ([4ad4964](https://github.com/lensapp/lens-sandbox/commit/4ad4964d0d8b7e250c512871804d48ee9c0b029c))
* make a directory's decisions the mixin the specification says they are ([e8d1450](https://github.com/lensapp/lens-sandbox/commit/e8d14508b5d5855c3bdbabc227d3089dd21a8640))
* name a document's egress where the specification names it ([3378778](https://github.com/lensapp/lens-sandbox/commit/33787787948b65a01964029d09912d03d7508190))
* **policy:** decide destinations with rules, not a default verdict ([e98b997](https://github.com/lensapp/lens-sandbox/commit/e98b9973d313bfeeacc9019af49253228a4ea662))
* provision declared tools pre-boot and prepend their bin paths to the workload PATH ([b58a110](https://github.com/lensapp/lens-sandbox/commit/b58a11057d67919f6658889d1c7253e2cc4d58cd))
* resolve every run's decisions as the directory's own mixin ([6b44283](https://github.com/lensapp/lens-sandbox/commit/6b44283286ebbb5f002ffaea8e0c1402852a9369))
* spell a document's kind the way the specification writes it ([69e2270](https://github.com/lensapp/lens-sandbox/commit/69e2270d01c2b172cdc8a32159e226b0edde3bd9))


### Bug Fixes

* **broker:** confine an exec session to the workload's identity ([be05478](https://github.com/lensapp/lens-sandbox/commit/be054784b5f7cff22f190f52d41659642c996f9c))
* **broker:** keep the relay credentials out of a confined session ([7d3cbd4](https://github.com/lensapp/lens-sandbox/commit/7d3cbd4e13f6361502f05928bffc445a04052799))
* claim a tool's source host only when the backend names one ([7785bc6](https://github.com/lensapp/lens-sandbox/commit/7785bc6a24d35cf144b7944ce3b68193796a4abb))
* give a workload the directories of its own tool trees ([3f3852a](https://github.com/lensapp/lens-sandbox/commit/3f3852aac85cc68c49ba6c3dadd89e2ef82ab530))
* give volume images a journal so an unclean shutdown cannot corrupt them ([45059f8](https://github.com/lensapp/lens-sandbox/commit/45059f8307f8115aa006261ef4a88f7c3ef30ec7))
* let a connector's own claim decide what it supplies ([d0b1848](https://github.com/lensapp/lens-sandbox/commit/d0b18485ea33d6310e63b1133a214a87c3196d29))
* let validate answer for the document every verb runs ([67ae060](https://github.com/lensapp/lens-sandbox/commit/67ae0608791a2ec6a2d6fc19d2aa30ef53ac9ce7))
* make the provisioner guest boot and install against a live guest ([b7961ba](https://github.com/lensapp/lens-sandbox/commit/b7961bafdc9cbcc7182ee5d07b73acfd58899c07))
* place a declared tool's bin dirs from the engine, not a guess ([95e9853](https://github.com/lensapp/lens-sandbox/commit/95e9853cd65c0f543b6644c062413488bf1b88fd))
* release each volume before the guest powers off ([b31efae](https://github.com/lensapp/lens-sandbox/commit/b31efae0171b13ddd1dd99ad721682c7018408ae))
* stop dispatch re-locking the stdin its caller already holds ([28ea217](https://github.com/lensapp/lens-sandbox/commit/28ea217642b6526cd46f5d317a412e691ce54bb3))
* **supervisor:** drop capabilities when the run-as user is root ([f7ae228](https://github.com/lensapp/lens-sandbox/commit/f7ae228ddb30d3afb1777fe9f1a6899fe72d9d84))
* **supervisor:** keep the run-as identity when the setuid is dropped ([fd9c17a](https://github.com/lensapp/lens-sandbox/commit/fd9c17a9e98956e799ffe4b663438d118336fc55))
* take a tool's whole install, not the dir the engine names ([c0cba37](https://github.com/lensapp/lens-sandbox/commit/c0cba377f14ca24f7ae828ef4b0a5b70c6769a98))


### Performance Improvements

* reuse latest pins after install lock ([30f3b14](https://github.com/lensapp/lens-sandbox/commit/30f3b14ed37ca18ef35fe3dc274df991a181100d))


### Code Refactoring

* **policy:** adopt egress.http as the canonical route table ([f302a01](https://github.com/lensapp/lens-sandbox/commit/f302a011b3931f3078d023fd3b48a58784b98a60))
</details>

<details><summary>lns-cli: 0.18.0</summary>

## [0.18.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.17.0...lns-cli-v0.18.0) (2026-08-14)


###   BREAKING CHANGES

* make a directory's decisions the mixin the specification says they are
* keep which connectors a project connected out of the file it commits
* a definition carries `name` at the top level, and a document that nests it under `metadata` is refused by name. A `labels` block is refused too.
* a definition names its egress under `spec.egress`. The old spelling is refused by name rather than accepted alongside it.
* a definition must spell its kind in lower case. Pre-1.0, so there is no shim: the old spelling is gone rather than accepted too.
* a sandbox declares the credential it needs, never a connector
* **policy:** decide destinations with rules, not a default verdict
* **policy:** adopt egress.http as the canonical route table
* **cli:** lns volume inspect renames its keys, size_bytes to sizeBytes, disk_bytes to diskBytes and in_use_by to inUseBy. A script reading the old names gets null rather than an error, so update it before upgrading.

### Features

* a sandbox declares the credential it needs, never a connector ([3527be3](https://github.com/lensapp/lens-sandbox/commit/3527be34f67f38f6badff1ed3c0420a5047f2233))
* add bump-mise operator tooling and move the Claude Code example onto spec.tools ([a9e33be](https://github.com/lensapp/lens-sandbox/commit/a9e33bedddbb62b346327a74996f82345efa80c6))
* attribute every entry of a resolved sandbox to the source that decided it ([c456539](https://github.com/lensapp/lens-sandbox/commit/c456539588f014f8e70973af4b97ee366c7b73ca))
* carry a document's name above its spec ([42a93af](https://github.com/lensapp/lens-sandbox/commit/42a93af73bb085de463494d18186e5b92bc6da4e))
* **cli:** give lns audit --format table|jsonl ([5ca1f7a](https://github.com/lensapp/lens-sandbox/commit/5ca1f7aba34db85e91e1a09e0167afc67acacdd4))
* **cli:** give lns config list and get --format json ([02ccb74](https://github.com/lensapp/lens-sandbox/commit/02ccb7487e0a85ede1517a1745735cb7450a437e))
* **cli:** give lns connector list and grants --format json ([4b8d5b1](https://github.com/lensapp/lens-sandbox/commit/4b8d5b1b729e1a7421ce7fb9d0efb2fec28da569))
* **cli:** give lns policy list --format json ([a8923d2](https://github.com/lensapp/lens-sandbox/commit/a8923d2b41c0782342192707032687503866e60b))
* **cli:** give lns ps --format json ([feff7fa](https://github.com/lensapp/lens-sandbox/commit/feff7fa94831bb258afa099cd623db0807ccf5f1))
* **cli:** give lns sandbox ls --format json ([4075a6e](https://github.com/lensapp/lens-sandbox/commit/4075a6e7e7efecdcf0ebd0cf709c8033653d5e2b))
* **cli:** give lns service status --format json ([55d8bbf](https://github.com/lensapp/lens-sandbox/commit/55d8bbf2265f6997bc4ea01ca1457cef0bcebfeb))
* **cli:** give lns volume ls --format json ([c7db0b7](https://github.com/lensapp/lens-sandbox/commit/c7db0b72f2317a2aa95e0e9cea38d05bf096b8e8))
* **cli:** inspect, revoke, and disconnect-clear per-workload grants ([a66f01d](https://github.com/lensapp/lens-sandbox/commit/a66f01d0e90e7fd7acfdebbd7478155c6ad997d8))
* **cli:** shared machine-readable output seam ([e7bb085](https://github.com/lensapp/lens-sandbox/commit/e7bb085d5104cae1c2a1e867367394dd4ad92228))
* **cli:** tell connect when this project holds a standing decline ([6b527d3](https://github.com/lensapp/lens-sandbox/commit/6b527d33f3c49fc99e2c54fa3b1f8aebc6f9e43a))
* declare developer tools via spec.tools with offline shape validation ([4ad4964](https://github.com/lensapp/lens-sandbox/commit/4ad4964d0d8b7e250c512871804d48ee9c0b029c))
* disclose declared tools in inspect and the run summary ([d4eb56f](https://github.com/lensapp/lens-sandbox/commit/d4eb56fbad4528531473843454e5bd8cb6c3a2bb))
* keep which connectors a project connected out of the file it commits ([2d7e808](https://github.com/lensapp/lens-sandbox/commit/2d7e808f569799cf35d121c4c6e98bea8f1c44ab))
* key a connector grant on the mixins a run is composed of ([101474f](https://github.com/lensapp/lens-sandbox/commit/101474f71ff1d39305d8bbe4522a85d33144809c))
* let a definition declare a host file or a home-anchored bind ([b42fa20](https://github.com/lensapp/lens-sandbox/commit/b42fa208e431017906fcee7fa8ceb4bbb9ac98eb))
* let a definition declare the user it needs to run as ([b19b61a](https://github.com/lensapp/lens-sandbox/commit/b19b61a7a5b502bdb4928e837efdee54a3b9700a))
* let a definition exclude subpaths from a bind ([ac0e42f](https://github.com/lensapp/lens-sandbox/commit/ac0e42f50cae781bfbff4184885da9f3526c8368))
* let a definition size itself as a share of the host ([6e5226f](https://github.com/lensapp/lens-sandbox/commit/6e5226fa71f3866aba912a613bd4657465be8deb))
* let a document declare the mixins it layers on ([b74b449](https://github.com/lensapp/lens-sandbox/commit/b74b4490f671e0179ccffcd5b9b8b1c8c2592e96))
* let a document read a mixin from a directory beside it ([50ddd83](https://github.com/lensapp/lens-sandbox/commit/50ddd83e3a0dc5eb36c19d7ab224073ec5b972c4))
* let a published mixin be pulled and inspected ([e75ae41](https://github.com/lensapp/lens-sandbox/commit/e75ae414318da7fd011a36a63cf78291426e337d))
* let a user merge their own mixins into a run ([f0b802b](https://github.com/lensapp/lens-sandbox/commit/f0b802bd6e37bde82c506502de7bc70f544e1844))
* make a directory's decisions the mixin the specification says they are ([e8d1450](https://github.com/lensapp/lens-sandbox/commit/e8d14508b5d5855c3bdbabc227d3089dd21a8640))
* name a document's egress where the specification names it ([3378778](https://github.com/lensapp/lens-sandbox/commit/33787787948b65a01964029d09912d03d7508190))
* pin resolved tool versions into the published artifact at push ([3a4489e](https://github.com/lensapp/lens-sandbox/commit/3a4489ef3be53ff3cb974940f77ae24cd0e89a2d))
* **policy:** decide destinations with rules, not a default verdict ([e98b997](https://github.com/lensapp/lens-sandbox/commit/e98b9973d313bfeeacc9019af49253228a4ea662))
* **policy:** per-binary scoping for network routes ([ae9d21b](https://github.com/lensapp/lens-sandbox/commit/ae9d21b7300ca822ce9044a8949b4092ae2748eb))
* **policy:** raw TCP egress with treatment-aware approvals ([d6c3951](https://github.com/lensapp/lens-sandbox/commit/d6c3951301d02ec929a01d6aa1b7ee6e6da392c3))
* publish an already-exact tool pin without the index ([69565de](https://github.com/lensapp/lens-sandbox/commit/69565def75b78e771cbcabea3a962ac45437e0a4))
* re-resolve [@latest](https://github.com/latest) tools against the index on every run ([280dc49](https://github.com/lensapp/lens-sandbox/commit/280dc493a12b3a80d317d23896711e73118778c9))
* record a pull's tool acquisition on the machine audit chain ([e7046aa](https://github.com/lensapp/lens-sandbox/commit/e7046aa01421aa6f7685ffd037ba77211cdfa5f6))
* report the tool versions a push pinned ([d689f0e](https://github.com/lensapp/lens-sandbox/commit/d689f0e1f03b8b31307d93cc9b951d5b1c347661))
* resolve a published sandbox's mixins before it boots ([562b1e2](https://github.com/lensapp/lens-sandbox/commit/562b1e2a47350641878414bef31ca5aa1b6952f9))
* resolve every run's decisions as the directory's own mixin ([6b44283](https://github.com/lensapp/lens-sandbox/commit/6b44283286ebbb5f002ffaea8e0c1402852a9369))
* **service:** gate connector arming on per-workload grants ([4012673](https://github.com/lensapp/lens-sandbox/commit/4012673b37edb008a0f1c7d0cfcd23750aa5d025))
* spell a document's kind the way the specification writes it ([69e2270](https://github.com/lensapp/lens-sandbox/commit/69e2270d01c2b172cdc8a32159e226b0edde3bd9))
* validate and disclose declared tools in the offline author verbs ([05d2907](https://github.com/lensapp/lens-sandbox/commit/05d29077d18c532326bf3fd8c9bc2f56f2afcedd))
* warn at push when the index does not list an exact tool pin ([f7ef447](https://github.com/lensapp/lens-sandbox/commit/f7ef447234d5a4c565bc1351fd1e227e028188aa))


### Bug Fixes

* bound the version-index query so the fallback fires ([691da53](https://github.com/lensapp/lens-sandbox/commit/691da533f494adddc141637a61bfb0b1b0a886bc))
* claim a tool's source host only when the backend names one ([7785bc6](https://github.com/lensapp/lens-sandbox/commit/7785bc6a24d35cf144b7944ce3b68193796a4abb))
* **cli:** clear per-workload grants before dropping a disconnected connector ([06b62d3](https://github.com/lensapp/lens-sandbox/commit/06b62d3bc3cafb657cf844fd200647c327ee953e))
* **cli:** default a bare registry reference to the Lens hub ([6aa8261](https://github.com/lensapp/lens-sandbox/commit/6aa82613e98d3af75c831b1cac91410f6ea81c22))
* **cli:** make ps report the same status shape as inspect ([8715bb6](https://github.com/lensapp/lens-sandbox/commit/8715bb6cccfecf31fb7af00c9551c667d79cfd56))
* closing an approval card no longer decides anything ([c3b0702](https://github.com/lensapp/lens-sandbox/commit/c3b0702b1b11857effbc8fac3432ba710963780f))
* disclose the rules a run enforces when it layered on nothing ([2411b84](https://github.com/lensapp/lens-sandbox/commit/2411b846a23873d2f938192f6708fdfc2d403d5c))
* gate unprovisionable tools at authoring, not on consume ([458f0cb](https://github.com/lensapp/lens-sandbox/commit/458f0cbdcd8301d3b60f4178f445c59d1e22956e))
* keep the ps listing when one guest stops answering ([d007bea](https://github.com/lensapp/lens-sandbox/commit/d007beadd02a33e47360a598885004dc258eba93))
* let a connector's own claim decide what it supplies ([d0b1848](https://github.com/lensapp/lens-sandbox/commit/d0b18485ea33d6310e63b1133a214a87c3196d29))
* let every prompting sandbox verb read the terminal it was invoked from ([06663cd](https://github.com/lensapp/lens-sandbox/commit/06663cd0300b2cc3d45b1a8e7de02fa23eb87c4c))
* let the flag read the memory size a definition writes ([58cf8cd](https://github.com/lensapp/lens-sandbox/commit/58cf8cdc941bf721b78ce3743be7e4d5d2ac4d4c))
* let validate answer for the document every verb runs ([67ae060](https://github.com/lensapp/lens-sandbox/commit/67ae0608791a2ec6a2d6fc19d2aa30ef53ac9ce7))
* make an exact tool request cache-addressable without the record ([af0e77e](https://github.com/lensapp/lens-sandbox/commit/af0e77e7e516f92c82c401b77186dbe301765762))
* make tool provisioning a filterable, honestly-typed audit event ([960a686](https://github.com/lensapp/lens-sandbox/commit/960a686b6c36dc8261a2b7e23d66a91c14dda0d9))
* **policy:** keep one catch-all rule, not one behind another ([ca602fd](https://github.com/lensapp/lens-sandbox/commit/ca602fd455376f8bbaca0bdef660458a5aaf84ab))
* **policy:** let a disconnect cancel a grant the run is still holding ([2afdf3c](https://github.com/lensapp/lens-sandbox/commit/2afdf3c29b5a8f9571ec7bf49094804c64cbdf93))
* **policy:** make the migration steps runnable and count raw rules at launch ([a343887](https://github.com/lensapp/lens-sandbox/commit/a34388735fce6f954eb7249f12eebd1b466c90a3))
* prune provisioned tool cache safely ([cd8fa24](https://github.com/lensapp/lens-sandbox/commit/cd8fa24e042fe420b88fa432cbd29d73a71b88cd))
* recognize resolver-emitted vendor versions as exact pins ([8297b99](https://github.com/lensapp/lens-sandbox/commit/8297b99b24125420bd5a9305844e6d1fc7a51061))
* refuse a single-file bind instead of booting a guest that dies ([96623b5](https://github.com/lensapp/lens-sandbox/commit/96623b5c199278abaa5352a0d4aaf36736879fc9))
* refuse two mappings on one host port before boot ([0f14181](https://github.com/lensapp/lens-sandbox/commit/0f141813cebae8184a7ff856adc62072b30312ff))
* **registry:** route every registry client through one loopback-aware transport ([653bc6d](https://github.com/lensapp/lens-sandbox/commit/653bc6d2308e2659e6ca148731a52861fc92d174))
* report the resources a run will boot with, not the flags ([5edb799](https://github.com/lensapp/lens-sandbox/commit/5edb799507a8f085818030f08981af7c397375e4))
* require consent for published tool installers ([902c26c](https://github.com/lensapp/lens-sandbox/commit/902c26c9796728481d5f559d989779b8671af9cc))
* root a definition's mixins at a directory the caller named ([709c0b6](https://github.com/lensapp/lens-sandbox/commit/709c0b666532a91c7ed03fe25b56d21e8c67692a))
* stop dispatch re-locking the stdin its caller already holds ([28ea217](https://github.com/lensapp/lens-sandbox/commit/28ea217642b6526cd46f5d317a412e691ce54bb3))
* stop offering `lns exec` a session it cannot open ([bc4b7bf](https://github.com/lensapp/lens-sandbox/commit/bc4b7bf3747ef3bc121ecdd2e6e7176e7d5ccb8b))
* stop the push index read at the cap instead of after it ([25f6927](https://github.com/lensapp/lens-sandbox/commit/25f69276912335cbf67de724678131b0df938450))
* surface pull-time tool warnings ([d6fa2b8](https://github.com/lensapp/lens-sandbox/commit/d6fa2b8914f1e952887cb784883097f7caf08122))
* validate dry-run tools before push preview ([2228ea8](https://github.com/lensapp/lens-sandbox/commit/2228ea8c559ea11778951a43e9a3ae52e4940525))
* warn about a differing digest only when a version is fuzzy ([7bf9060](https://github.com/lensapp/lens-sandbox/commit/7bf906089e082d3715820fb0f9f4c9798ac687ca))


### Performance Improvements

* reuse latest pins after install lock ([30f3b14](https://github.com/lensapp/lens-sandbox/commit/30f3b14ed37ca18ef35fe3dc274df991a181100d))


### Code Refactoring

* **policy:** adopt egress.http as the canonical route table ([f302a01](https://github.com/lensapp/lens-sandbox/commit/f302a011b3931f3078d023fd3b48a58784b98a60))
</details>

<details><summary>lns-service: 0.18.0</summary>

## [0.18.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.17.0...lns-service-v0.18.0) (2026-08-14)


###   BREAKING CHANGES

* make a directory's decisions the mixin the specification says they are
* keep which connectors a project connected out of the file it commits
* a definition carries `name` at the top level, and a document that nests it under `metadata` is refused by name. A `labels` block is refused too.
* a definition names its egress under `spec.egress`. The old spelling is refused by name rather than accepted alongside it.
* let the later source decide where two disagree about a destination
* a definition must spell its kind in lower case. Pre-1.0, so there is no shim: the old spelling is gone rather than accepted too.
* a sandbox declares the credential it needs, never a connector
* **policy:** decide destinations with rules, not a default verdict
* **policy:** adopt egress.http as the canonical route table

### Features

* a sandbox declares the credential it needs, never a connector ([3527be3](https://github.com/lensapp/lens-sandbox/commit/3527be34f67f38f6badff1ed3c0420a5047f2233))
* **a11y:** label the approval window's dismiss controls ([dbd6fa0](https://github.com/lensapp/lens-sandbox/commit/dbd6fa0fce0e7205a109c2f1704aa94edcaac102))
* add bump-mise operator tooling and move the Claude Code example onto spec.tools ([a9e33be](https://github.com/lensapp/lens-sandbox/commit/a9e33bedddbb62b346327a74996f82345efa80c6))
* attribute every entry of a resolved sandbox to the source that decided it ([c456539](https://github.com/lensapp/lens-sandbox/commit/c456539588f014f8e70973af4b97ee366c7b73ca))
* cache provisioned tool trees and record resolved versions per machine ([e2cffe5](https://github.com/lensapp/lens-sandbox/commit/e2cffe5e222570fafa0ab3b0ef24c120ba51baaa))
* carry a document's name above its spec ([42a93af](https://github.com/lensapp/lens-sandbox/commit/42a93af73bb085de463494d18186e5b92bc6da4e))
* compose declared tools from cache, record, and provisioner with first-resolution pinning ([7c0acfc](https://github.com/lensapp/lens-sandbox/commit/7c0acfca0eb403095c7a92fc704e9de190c3ea3a))
* declare developer tools via spec.tools with offline shape validation ([4ad4964](https://github.com/lensapp/lens-sandbox/commit/4ad4964d0d8b7e250c512871804d48ee9c0b029c))
* detect the workload image's libc flavor from its layer tars ([364e00e](https://github.com/lensapp/lens-sandbox/commit/364e00e4f7f2cfb1aef9e15ce35bfdba94b175be))
* disclose declared tools in inspect and the run summary ([d4eb56f](https://github.com/lensapp/lens-sandbox/commit/d4eb56fbad4528531473843454e5bd8cb6c3a2bb))
* keep which connectors a project connected out of the file it commits ([2d7e808](https://github.com/lensapp/lens-sandbox/commit/2d7e808f569799cf35d121c4c6e98bea8f1c44ab))
* key a connector grant on the mixins a run is composed of ([101474f](https://github.com/lensapp/lens-sandbox/commit/101474f71ff1d39305d8bbe4522a85d33144809c))
* let a definition declare a host file or a home-anchored bind ([b42fa20](https://github.com/lensapp/lens-sandbox/commit/b42fa208e431017906fcee7fa8ceb4bbb9ac98eb))
* let a definition declare the user it needs to run as ([b19b61a](https://github.com/lensapp/lens-sandbox/commit/b19b61a7a5b502bdb4928e837efdee54a3b9700a))
* let a definition exclude subpaths from a bind ([ac0e42f](https://github.com/lensapp/lens-sandbox/commit/ac0e42f50cae781bfbff4184885da9f3526c8368))
* let a definition size itself as a share of the host ([6e5226f](https://github.com/lensapp/lens-sandbox/commit/6e5226fa71f3866aba912a613bd4657465be8deb))
* let a document declare the mixins it layers on ([b74b449](https://github.com/lensapp/lens-sandbox/commit/b74b4490f671e0179ccffcd5b9b8b1c8c2592e96))
* let a document read a mixin from a directory beside it ([50ddd83](https://github.com/lensapp/lens-sandbox/commit/50ddd83e3a0dc5eb36c19d7ab224073ec5b972c4))
* let a published mixin be pulled and inspected ([e75ae41](https://github.com/lensapp/lens-sandbox/commit/e75ae414318da7fd011a36a63cf78291426e337d))
* let a resolution carry the directory's own decisions as its last source ([22cbc13](https://github.com/lensapp/lens-sandbox/commit/22cbc139e107b2ed025a80a772bb90d6000cd91d))
* let a user merge their own mixins into a run ([f0b802b](https://github.com/lensapp/lens-sandbox/commit/f0b802bd6e37bde82c506502de7bc70f544e1844))
* let the later source decide where two disagree about a destination ([7a85d9a](https://github.com/lensapp/lens-sandbox/commit/7a85d9a2b4eb380b2b35efc42d327b627948c488))
* make a directory's decisions the mixin the specification says they are ([e8d1450](https://github.com/lensapp/lens-sandbox/commit/e8d14508b5d5855c3bdbabc227d3089dd21a8640))
* name a document's egress where the specification names it ([3378778](https://github.com/lensapp/lens-sandbox/commit/33787787948b65a01964029d09912d03d7508190))
* pin the mise engine, provisioner rootfs images, and companion artifacts ([8067146](https://github.com/lensapp/lens-sandbox/commit/80671467fefbf6262744e2db8a79c7ed6310a36c))
* **policy:** decide destinations with rules, not a default verdict ([e98b997](https://github.com/lensapp/lens-sandbox/commit/e98b9973d313bfeeacc9019af49253228a4ea662))
* **policy:** per-binary scoping for network routes ([ae9d21b](https://github.com/lensapp/lens-sandbox/commit/ae9d21b7300ca822ce9044a8949b4092ae2748eb))
* **policy:** raw TCP egress with treatment-aware approvals ([d6c3951](https://github.com/lensapp/lens-sandbox/commit/d6c3951301d02ec929a01d6aa1b7ee6e6da392c3))
* pre-provision a pulled sandbox's pinned tools so it starts offline ([6ae5e5f](https://github.com/lensapp/lens-sandbox/commit/6ae5e5fc397e4d33cfc01f83cf69498a8d6bb16e))
* provision declared tools in a disposable engine guest with a writable staging share ([6321296](https://github.com/lensapp/lens-sandbox/commit/6321296d3941c83dfb3cb0df066bd743474e15e6))
* provision declared tools pre-boot and prepend their bin paths to the workload PATH ([b58a110](https://github.com/lensapp/lens-sandbox/commit/b58a11057d67919f6658889d1c7253e2cc4d58cd))
* re-resolve [@latest](https://github.com/latest) tools against the index on every run ([280dc49](https://github.com/lensapp/lens-sandbox/commit/280dc493a12b3a80d317d23896711e73118778c9))
* record a pull's tool acquisition on the machine audit chain ([e7046aa](https://github.com/lensapp/lens-sandbox/commit/e7046aa01421aa6f7685ffd037ba77211cdfa5f6))
* record tool provisioning in the run's audit chain ([5db2a73](https://github.com/lensapp/lens-sandbox/commit/5db2a7357ce1a5810f35194825ad1f4fc9b6318e))
* refuse unknown and plugin-backed tools against the pinned registry snapshot ([18ddc23](https://github.com/lensapp/lens-sandbox/commit/18ddc2387ee9900d58b7d2e0163846c4c03606d5))
* refuse unprovisionable tools at authoring time ([ae81794](https://github.com/lensapp/lens-sandbox/commit/ae817947d6a4b3830b4874a44ed4de17824eca8a))
* resolve a published sandbox's mixins before it boots ([562b1e2](https://github.com/lensapp/lens-sandbox/commit/562b1e2a47350641878414bef31ca5aa1b6952f9))
* resolve every run's decisions as the directory's own mixin ([6b44283](https://github.com/lensapp/lens-sandbox/commit/6b44283286ebbb5f002ffaea8e0c1402852a9369))
* reuse a tool tree only where its guest-mates are trusted ([5197c70](https://github.com/lensapp/lens-sandbox/commit/5197c70dd51d20abad4c0cc6a493288f9adda1d6))
* **service:** gate connector arming on per-workload grants ([4012673](https://github.com/lensapp/lens-sandbox/commit/4012673b37edb008a0f1c7d0cfcd23750aa5d025))
* **service:** grant a value already bound on this machine from the card ([d804661](https://github.com/lensapp/lens-sandbox/commit/d804661a3f6982c15db37c1ec39f569293e1bd6d))
* **service:** offer a reconnect alongside spending a bound connection ([4e3f31a](https://github.com/lensapp/lens-sandbox/commit/4e3f31a4a05a43a51893bcd0c593e8f9bae02aa9))
* **service:** record per-workload connector grants at consent ([45b0405](https://github.com/lensapp/lens-sandbox/commit/45b0405d84d09b2155329f9bcb0bdfe24b054995))
* **service:** remember a declined connector as a per-workload deny ([a3a5772](https://github.com/lensapp/lens-sandbox/commit/a3a57727ab089ca776dfc915dcf34cf416d871f3))
* spell a document's kind the way the specification writes it ([69e2270](https://github.com/lensapp/lens-sandbox/commit/69e2270d01c2b172cdc8a32159e226b0edde3bd9))


### Bug Fixes

* a closed network card reads as undecided on the wire ([3943ac3](https://github.com/lensapp/lens-sandbox/commit/3943ac3b0753e813a703a2d626d27cd4aa5f4a60))
* allowlist an index-resolved version before it becomes a path ([00c0b66](https://github.com/lensapp/lens-sandbox/commit/00c0b66d5f67c45bbf699a71ffe73440b55952f1))
* **artifact:** refuse two credential slots naming one connector ([fcd8a29](https://github.com/lensapp/lens-sandbox/commit/fcd8a29abd4532dd2fa204a012c9b223341867e4))
* bound staged tool tar ingestion ([8325545](https://github.com/lensapp/lens-sandbox/commit/8325545a4d2863738685d71ab1bf237cd4897023))
* bound the provisioner's stderr instead of failing on it ([30d9101](https://github.com/lensapp/lens-sandbox/commit/30d910171f39e9ead3f02499738abf4d2b8cb4ac))
* bound the version-index query so the fallback fires ([691da53](https://github.com/lensapp/lens-sandbox/commit/691da533f494adddc141637a61bfb0b1b0a886bc))
* bound the workload trust-store fetch ([42f7248](https://github.com/lensapp/lens-sandbox/commit/42f72485cce3ce7b879f00aec852218bb8c8db4b))
* **broker:** confine an exec session to the workload's identity ([be05478](https://github.com/lensapp/lens-sandbox/commit/be054784b5f7cff22f190f52d41659642c996f9c))
* **broker:** keep the relay credentials out of a confined session ([7d3cbd4](https://github.com/lensapp/lens-sandbox/commit/7d3cbd4e13f6361502f05928bffc445a04052799))
* cache virtiofsd capability checks ([a3acefd](https://github.com/lensapp/lens-sandbox/commit/a3acefd3a2e434edf3e46f851a979d00f9fbcbb5))
* carry the version allowlist in the type that reaches the sinks ([4d9b8c9](https://github.com/lensapp/lens-sandbox/commit/4d9b8c94793bb6f4d48a8d473ee2f141cd081ad2))
* claim a tool's source host only when the backend names one ([7785bc6](https://github.com/lensapp/lens-sandbox/commit/7785bc6a24d35cf144b7944ce3b68193796a4abb))
* closing an approval card no longer decides anything ([c3b0702](https://github.com/lensapp/lens-sandbox/commit/c3b0702b1b11857effbc8fac3432ba710963780f))
* disclose each tool as it commits, not after the set survives ([ee5c473](https://github.com/lensapp/lens-sandbox/commit/ee5c4731b211f30bc4d3049b1a010152fa882935))
* disclose the rules a run enforces when it layered on nothing ([2411b84](https://github.com/lensapp/lens-sandbox/commit/2411b846a23873d2f938192f6708fdfc2d403d5c))
* distinguish co-installed musl loaders ([d2a5844](https://github.com/lensapp/lens-sandbox/commit/d2a5844eff2e99d67292f24926d7912afab35a8c))
* give a workload the directories of its own tool trees ([3f3852a](https://github.com/lensapp/lens-sandbox/commit/3f3852aac85cc68c49ba6c3dadd89e2ef82ab530))
* give each tool its own provisioner engine state ([6392cbb](https://github.com/lensapp/lens-sandbox/commit/6392cbbe1303517d55f64be4165f6a2a48ac336f))
* give every workload guest a public trust store ([229b1e8](https://github.com/lensapp/lens-sandbox/commit/229b1e868c9e75d06a5960f13f1df74b1e27b678))
* give lns exec the run's own tool PATH ([b7d00f8](https://github.com/lensapp/lens-sandbox/commit/b7d00f8976b8ee06bb8d33353133c4c168976536))
* give the driver sole ownership of the marker channel ([4df06c4](https://github.com/lensapp/lens-sandbox/commit/4df06c42d83a40c88f7ccd9354330cab7dc59c6c))
* give volume images a journal so an unclean shutdown cannot corrupt them ([45059f8](https://github.com/lensapp/lens-sandbox/commit/45059f8307f8115aa006261ef4a88f7c3ef30ec7))
* honor OCI whiteouts in libc detection ([3121a9b](https://github.com/lensapp/lens-sandbox/commit/3121a9bb7ce759f6fd8ff625764e3425d117e2db))
* honor the record and manifest schema versions on load ([522348c](https://github.com/lensapp/lens-sandbox/commit/522348c8fde1089e1fd57ed0c5a5d8311537a4d1))
* inject only the companion libs a tool links ([0a5a04d](https://github.com/lensapp/lens-sandbox/commit/0a5a04da5254ddae3fb4e442d8e90505b01b4ce5))
* keep a blank PATH segment out of every workload, not just a tooled one ([368e07b](https://github.com/lensapp/lens-sandbox/commit/368e07b0f19bf126db3cda52428e321e2b2888a0))
* keep a warm tool set out of the install queue ([0838b9a](https://github.com/lensapp/lens-sandbox/commit/0838b9a7ff7c9ec48ea091b249a2efe57cd20e6a))
* keep blank segments off the workload PATH ([1c014aa](https://github.com/lensapp/lens-sandbox/commit/1c014aa7147da981c287f794dcb9d0027e19eedd))
* keep the bin-dir query's diagnostics for the failure cause ([62951d2](https://github.com/lensapp/lens-sandbox/commit/62951d27c56e59d8a0704f0aef25fb8fd0f40eec))
* keep the engine off the warm musl path ([dc1477b](https://github.com/lensapp/lens-sandbox/commit/dc1477b9e292097e9d75c9a657c63b208b299411))
* keep the guest stats probe's capture stdout-only ([8b0841f](https://github.com/lensapp/lens-sandbox/commit/8b0841fd41a023035153968a6b5f374413ce7b7f))
* keep the tool resolution record when prune reclaims the tool cache ([fd88e03](https://github.com/lensapp/lens-sandbox/commit/fd88e03fef9b4186ec8356f63784dba41ac739e4))
* let a ceiling permit what its own patterns cover ([29aeeef](https://github.com/lensapp/lens-sandbox/commit/29aeeef5cf469230d0873f2362907c1d2430b722))
* let a connector's own claim decide what it supplies ([d0b1848](https://github.com/lensapp/lens-sandbox/commit/d0b18485ea33d6310e63b1133a214a87c3196d29))
* let a definition's HOME survive the run-as user ([0c6b286](https://github.com/lensapp/lens-sandbox/commit/0c6b2868fc9245a14e02619d307ac517ad5d81bd))
* let a pull survive a failed tool pre-provision ([80c30f8](https://github.com/lensapp/lens-sandbox/commit/80c30f825ff3f8823b5e92effd9239ae80710f0e))
* let a vendor-prefixed version resolve at push ([e3cff2a](https://github.com/lensapp/lens-sandbox/commit/e3cff2a7d050a11c9b4e9713093bae4a8685b2ea))
* let an exec session join the run it names ([07e2116](https://github.com/lensapp/lens-sandbox/commit/07e2116a034f843406fdee8d8c5508e828559941))
* let no later layer overturn a musl loader ([e686b73](https://github.com/lensapp/lens-sandbox/commit/e686b73d551ad4d81c6f5ad2a10d5d7f529ea78f))
* let the flag read the memory size a definition writes ([58cf8cd](https://github.com/lensapp/lens-sandbox/commit/58cf8cdc941bf721b78ce3743be7e4d5d2ac4d4c))
* make an exact tool request cache-addressable without the record ([af0e77e](https://github.com/lensapp/lens-sandbox/commit/af0e77e7e516f92c82c401b77186dbe301765762))
* make guest content shares read-only ([f735aaa](https://github.com/lensapp/lens-sandbox/commit/f735aaa8b17b57f567f4124a9f17948b3be7c301))
* make pull provisioning retry warning neutral ([b5434a2](https://github.com/lensapp/lens-sandbox/commit/b5434a2e8ac126bbcf487553cc29e6d4ae6d10ae))
* make the provisioner guest boot and install against a live guest ([b7961ba](https://github.com/lensapp/lens-sandbox/commit/b7961bafdc9cbcc7182ee5d07b73acfd58899c07))
* make the snapshot's backend the one mise actually installs from ([ff56ce5](https://github.com/lensapp/lens-sandbox/commit/ff56ce5e893f6ffa2ac7ef8c88a509d2b7cd07c9))
* make the tool record durable and its neighbours serialized ([aef4586](https://github.com/lensapp/lens-sandbox/commit/aef4586a1f89b333df1e6be69728491e24b555ee))
* make tool provisioning a filterable, honestly-typed audit event ([960a686](https://github.com/lensapp/lens-sandbox/commit/960a686b6c36dc8261a2b7e23d66a91c14dda0d9))
* map every writable bind, including the root provisioner's ([9fa21bf](https://github.com/lensapp/lens-sandbox/commit/9fa21bf91766054eecb4acc9bf741b22c81be522))
* measure a tool symlink against the path injection will create ([3b477fb](https://github.com/lensapp/lens-sandbox/commit/3b477fbd45808a6a3a2920d08c4961546525f1a7))
* move tool tar ingest off the async worker ([0099fa1](https://github.com/lensapp/lens-sandbox/commit/0099fa1769a1c56ef99a6b5f4b837e5fae59a392))
* parse the provisioner's markers from stdout alone ([8083b7a](https://github.com/lensapp/lens-sandbox/commit/8083b7a04e35f12f7073496e0490a8fe9212bebb))
* pin the CA store to a snapshot upstream keeps ([2db2c31](https://github.com/lensapp/lens-sandbox/commit/2db2c3178d246eb61db10e9b2e53d3546b8e284d))
* place a declared tool's bin dirs from the engine, not a guess ([95e9853](https://github.com/lensapp/lens-sandbox/commit/95e9853cd65c0f543b6644c062413488bf1b88fd))
* **policy:** let a disconnect cancel a grant the run is still holding ([2afdf3c](https://github.com/lensapp/lens-sandbox/commit/2afdf3c29b5a8f9571ec7bf49094804c64cbdf93))
* prune provisioned tool cache safely ([cd8fa24](https://github.com/lensapp/lens-sandbox/commit/cd8fa24e042fe420b88fa432cbd29d73a71b88cd))
* publish a run's tool paths with the gate that exposes them ([ddda302](https://github.com/lensapp/lens-sandbox/commit/ddda302c9478b5f8a77a020d188dc44d5f696543))
* re-apply ingest's tar guards to a manifest read off disk ([72cf5cb](https://github.com/lensapp/lens-sandbox/commit/72cf5cbe26f158eba5df7dadead01bf19b7036f7))
* re-apply the tree guards to a manifest read off disk ([e1ab69f](https://github.com/lensapp/lens-sandbox/commit/e1ab69fd6cde7cf9cdcfdfe35aaedc281370bc20))
* reap a provisioner guest that never became reachable ([e5f6eef](https://github.com/lensapp/lens-sandbox/commit/e5f6eeffb1e14e10c71a91607f8185d7f669d001))
* recognize resolver-emitted vendor versions as exact pins ([8297b99](https://github.com/lensapp/lens-sandbox/commit/8297b99b24125420bd5a9305844e6d1fc7a51061))
* refuse a provisioner run that reports one tool twice ([7d43298](https://github.com/lensapp/lens-sandbox/commit/7d432982f6922c046ee58da84503b065f0f86489))
* refuse a run that would boot unsupervised ([fc2ee7a](https://github.com/lensapp/lens-sandbox/commit/fc2ee7a9de374e8a758cc45fec22a03a95abf7ff))
* refuse a staged tool tar that is not the regular file we wrote ([454ba27](https://github.com/lensapp/lens-sandbox/commit/454ba27e5be55964c36e188cc37805d5a67d6463))
* refuse a tool entry that lives under a symlink ([4301a51](https://github.com/lensapp/lens-sandbox/commit/4301a519122f89610b3ae0c9606f454ea8dfb94b))
* refuse a tool symlink that points out of its own tree ([a8680e7](https://github.com/lensapp/lens-sandbox/commit/a8680e7bdc10d25515d176f79c6e6cbc838d594b))
* refuse a tool symlink whose double slash hides a level ([4ef312a](https://github.com/lensapp/lens-sandbox/commit/4ef312a58e82394161910c50261a1787c3afd433))
* refuse driver-reported tool locations that escape the cache tree ([2d663d0](https://github.com/lensapp/lens-sandbox/commit/2d663d0692efca44a87a5e7b21aeb4d2677bf0df))
* **registry:** route every registry client through one loopback-aware transport ([653bc6d](https://github.com/lensapp/lens-sandbox/commit/653bc6d2308e2659e6ca148731a52861fc92d174))
* reject virtiofsd without readonly support ([37f37ce](https://github.com/lensapp/lens-sandbox/commit/37f37ce2143eb8e434459a45c4a2e67cd84853f4))
* release image cache lock before tool provisioning ([81ff895](https://github.com/lensapp/lens-sandbox/commit/81ff8957d952a09c0774eea1e5b34069dad89338))
* report the resources a run will boot with, not the flags ([5edb799](https://github.com/lensapp/lens-sandbox/commit/5edb799507a8f085818030f08981af7c397375e4))
* require consent for published tool installers ([902c26c](https://github.com/lensapp/lens-sandbox/commit/902c26c9796728481d5f559d989779b8671af9cc))
* resolve a hardlinked tool file to its ingested digest ([364a7b4](https://github.com/lensapp/lens-sandbox/commit/364a7b49c9af1b62b0d858f506d210ef23dde4d1))
* resolve the run before looking up its tool PATH for exec ([a4be69c](https://github.com/lensapp/lens-sandbox/commit/a4be69c2a3f7993511111b9dddd192f39d979b7b))
* restore the stock npm launcher in provisioned tool trees ([b5c7ef9](https://github.com/lensapp/lens-sandbox/commit/b5c7ef9eea661a66787e15e23909c13ccbfbee24))
* reuse co-installed latest tool trees ([88ca7f5](https://github.com/lensapp/lens-sandbox/commit/88ca7f536f94f814bf7007b2b13e79d216850446))
* root a definition's mixins at a directory the caller named ([709c0b6](https://github.com/lensapp/lens-sandbox/commit/709c0b666532a91c7ed03fe25b56d21e8c67692a))
* **run:** confine the primary session when the run is unsupervised ([d4f586d](https://github.com/lensapp/lens-sandbox/commit/d4f586d95371a7a7236cec8b2bd3b42a72105a29))
* **service:** canonicalize a definition's workload identity directory ([75844d6](https://github.com/lensapp/lens-sandbox/commit/75844d697c1c476eaf565d95b12a83dc0cdd5d16))
* **service:** close four gaps in the per-workload grant lifecycle ([a277c1a](https://github.com/lensapp/lens-sandbox/commit/a277c1ad8c468377657c61edc3a128203b61f555))
* **service:** grant a required slot's boot sign-in to its workload ([c3e0a4d](https://github.com/lensapp/lens-sandbox/commit/c3e0a4da21467f43264f0cfae9169ee874c1e787))
* **service:** let a disconnect cancel a boot sign-in's grant too ([8c34e23](https://github.com/lensapp/lens-sandbox/commit/8c34e233f8ba39b82c161e4066d54effee31f41c))
* **service:** offer a bound value only when one is actually bound ([56f5977](https://github.com/lensapp/lens-sandbox/commit/56f5977cd6cc89524e744650b409c97bbc6b4a1c))
* **service:** pin a grant decision to the forget count its card was asked against ([8be2654](https://github.com/lensapp/lens-sandbox/commit/8be2654e4cf819a278daebb172cf5f00890a3ee9))
* **service:** point an unidentifiable run at the stale-service restart ([930e688](https://github.com/lensapp/lens-sandbox/commit/930e688dc5e251b573b83186ad3155aaa8512444))
* **service:** refuse a run that resolves no workload identity ([70edc30](https://github.com/lensapp/lens-sandbox/commit/70edc30037994dc31bc26fdb1857a7c6a969b8a0))
* **service:** revalidate deny grants and fix remember_grant ordering ([b050418](https://github.com/lensapp/lens-sandbox/commit/b0504188facd005f1f6cf414787d3221fcb799c7))
* size pruned caches from metadata ([136c5aa](https://github.com/lensapp/lens-sandbox/commit/136c5aa413bbaf82bc17e725c75aa6fc2a9f2023))
* skip the libc memo when the layer set has no digests ([99b5d4c](https://github.com/lensapp/lens-sandbox/commit/99b5d4cccd540f30d22d23b4687e7f5fe089e02a))
* stop a credential slot withdrawing a granted route ([7d97f3f](https://github.com/lensapp/lens-sandbox/commit/7d97f3f9ac40132fdeff08f2883fbd20d78ad89b))
* stop dispatch re-locking the stdin its caller already holds ([28ea217](https://github.com/lensapp/lens-sandbox/commit/28ea217642b6526cd46f5d317a412e691ce54bb3))
* stop guessing aqua download hosts ([06fd256](https://github.com/lensapp/lens-sandbox/commit/06fd2563067b1abbad86eb6a506fc4c2fa9b36a3))
* stop the provisioner guest before its trees are named final ([d9b65a2](https://github.com/lensapp/lens-sandbox/commit/d9b65a276f1301f0c083516bd573b6c214f31325))
* stop the push index read at the cap instead of after it ([25f6927](https://github.com/lensapp/lens-sandbox/commit/25f69276912335cbf67de724678131b0df938450))
* surface pull-time tool warnings ([d6fa2b8](https://github.com/lensapp/lens-sandbox/commit/d6fa2b8914f1e952887cb784883097f7caf08122))
* take a tool's whole install, not the dir the engine names ([c0cba37](https://github.com/lensapp/lens-sandbox/commit/c0cba377f14ca24f7ae828ef4b0a5b70c6769a98))
* warn about a differing digest only when a version is fuzzy ([7bf9060](https://github.com/lensapp/lens-sandbox/commit/7bf906089e082d3715820fb0f9f4c9798ac687ca))
* write only the developer's own policy back ([b1a7ccd](https://github.com/lensapp/lens-sandbox/commit/b1a7ccd3b6538be1294fc1144a9b22fb0781a7af))


### Performance Improvements

* keep the image libc scan off the async worker ([918545a](https://github.com/lensapp/lens-sandbox/commit/918545ab40ed2d6b0a92435fff10255ddfbdf54a))
* keep warm musl runs out of install queue ([aa2581e](https://github.com/lensapp/lens-sandbox/commit/aa2581ea4619cce8f100c15b7191913605a9c43a))
* memoize the image's libc flavor by its layer digests ([e1d0903](https://github.com/lensapp/lens-sandbox/commit/e1d0903522b1367667a4b89d037d0078c2d99aa7))
* resolve latest tool pins concurrently ([a3482cf](https://github.com/lensapp/lens-sandbox/commit/a3482cfa9a1c515752f9fd5ef7a4ef32847154a0))
* reuse latest pins after install lock ([30f3b14](https://github.com/lensapp/lens-sandbox/commit/30f3b14ed37ca18ef35fe3dc274df991a181100d))
* scan the pulled image's layers for libc in place ([6133303](https://github.com/lensapp/lens-sandbox/commit/6133303c98d4a29bacfa9c12387fe0b4cce05898))
* warm the workload trust store during concurrent prepare ([4dfe36b](https://github.com/lensapp/lens-sandbox/commit/4dfe36b0ee01dcf465d4a68ae20607783a1b47de))


### Code Refactoring

* **policy:** adopt egress.http as the canonical route table ([f302a01](https://github.com/lensapp/lens-sandbox/commit/f302a011b3931f3078d023fd3b48a58784b98a60))
</details>

---
This PR was generated with [Release Please](https://github.com/googleapis/release-please). See [documentation](https://github.com/googleapis/release-please#release-please).