# Changelog

## [0.21.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.20.0...lns-v0.21.0) (2026-08-26)


### ⚠ BREAKING CHANGES

* keep an ambiguous run prefix out of rm and inspect arbitration
* make the service own the registry login store
* refuse unsafe LNS_HOME roots and show what --purge deletes
* keep base images out of the artifact surfaces
* **lns-session-broker:** keep 126 for a found command whose interpreter is missing
* **lns-cli:** answer 125 for a run that exits before its session is ready
* **lns-cli:** give ls one spelling — the list alias is removed
* **lns-cli:** let the inspect shortcut carry both namespaces' flags
* **lns-cli:** answer 125 for a pre-start failure in the sandbox spellings of run and exec
* **lns-cli:** give the CLI the stopped sandbox — start, rm -f, ls -a, prune
* give the CLI docker-start parity — start, rm for runs, rmi for images
* answer lns start over the wire, failing closed on every conflict
* **lns-cli:** split `lns artifact` from `lns sandbox`
* one directory, ~/.lns, and LNS_HOME to move it
* **lns-cli:** give the inspect verbs a --format, and run/exec the exit codes §5 names
* **lns-cli:** cut `lns run` to the flags §3.2.1 names
* **lns-cli:** drop `lns policy`, leaving the run's prompt and the file

### Features

* a stopped run keeps the image it will boot from ([fe3cff8](https://github.com/lensapp/lens-sandbox/commit/fe3cff893113e41feb499b173922c4ef656fdc23))
* answer lns start over the wire, failing closed on every conflict ([647aed6](https://github.com/lensapp/lens-sandbox/commit/647aed68a884d77b0570c105a2a8725aae638bfb))
* audit the whole run lifecycle on one chain that outlives the run ([61bc1e5](https://github.com/lensapp/lens-sandbox/commit/61bc1e5fc02b9f9e6f70444a6a3744b0ccd4aa17))
* boot a stopped run back onto its preserved writable layer ([99a1c51](https://github.com/lensapp/lens-sandbox/commit/99a1c51aedc0bc782e1ab5734a5e31993d679603))
* disclose a run's pre-start scripts before it boots ([4ee47ad](https://github.com/lensapp/lens-sandbox/commit/4ee47adde056ab378e991f446bbb3f1bfbe841ef))
* give the CLI docker-start parity — start, rm for runs, rmi for images ([ee55608](https://github.com/lensapp/lens-sandbox/commit/ee55608c44095e7a0f86f34184009c7f2665f996))
* keep base images out of the artifact surfaces ([4090961](https://github.com/lensapp/lens-sandbox/commit/4090961c918ee47616373c7eec55f930085e9d99))
* let the registry hold a run that is stopped, not just one that is alive ([5862742](https://github.com/lensapp/lens-sandbox/commit/58627420bda5eb0d03808c386ef40885890da87b))
* **lns-cli:** close the artifact-verb gaps §3.1 names ([0479211](https://github.com/lensapp/lens-sandbox/commit/0479211859ae77306e2ff95f187cd5aa1ebbd09e))
* **lns-cli:** cut `lns run` to the flags §3.2.1 names ([8b427f4](https://github.com/lensapp/lens-sandbox/commit/8b427f473e77edec9c09f8b0aab0cf516dbd014b))
* **lns-cli:** drop `lns policy`, leaving the run's prompt and the file ([b9e22fa](https://github.com/lensapp/lens-sandbox/commit/b9e22faf8c160d5b77c94ced43bd5666f6772ba5)), closes [#187](https://github.com/lensapp/lens-sandbox/issues/187)
* **lns-cli:** give the CLI the stopped sandbox — start, rm -f, ls -a, prune ([18d4fe7](https://github.com/lensapp/lens-sandbox/commit/18d4fe7332da37c5df9c08ce381197a847ec7c87))
* **lns-cli:** give the inspect verbs a --format, and run/exec the exit codes §5 names ([8106070](https://github.com/lensapp/lens-sandbox/commit/8106070c5130c54d47a7bc481166eb50f03e8978))
* **lns-cli:** plan the local-mixin subtree a push publishes ([0b1302a](https://github.com/lensapp/lens-sandbox/commit/0b1302a70dc745220191cfca4f5b4c84e34ae529))
* **lns-cli:** publish a sandbox's local mixins before the sandbox itself ([f8553ba](https://github.com/lensapp/lens-sandbox/commit/f8553ba11acf8f2b8124bb76dd8e3d321625af0b))
* **lns-cli:** put `lns exec` and `lns start` on the front page ([fc65d2b](https://github.com/lensapp/lens-sandbox/commit/fc65d2b9ddd1ba1a900e7252fb011a79bfff32a9))
* **lns-cli:** say which command takes a document when a RUN verb gets one ([0fe0b79](https://github.com/lensapp/lens-sandbox/commit/0fe0b790421bf5068ed2042ca0328233d6b2132f))
* **lns-cli:** split `lns artifact` from `lns sandbox` ([f321348](https://github.com/lensapp/lens-sandbox/commit/f3213483f406628006f3ce3b873339985b1e7253))
* one directory, ~/.lns, and LNS_HOME to move it ([c7563b6](https://github.com/lensapp/lens-sandbox/commit/c7563b628374abfc6873c92aa4506796dbf7432a))
* publish README.md beside the document as a text/markdown layer ([c171fcf](https://github.com/lensapp/lens-sandbox/commit/c171fcf22d7ae92749741c17d69e6a975ae477cc))
* read and stage a document's pre-start scripts ([8483891](https://github.com/lensapp/lens-sandbox/commit/848389106d4bae84a0b4ae5feaeefa4ecaceb04b))
* reclaim a run's disk when its run leaves the registry ([ad53876](https://github.com/lensapp/lens-sandbox/commit/ad53876a619e1cfaade705c7db45fd643d633a8b))
* record pulled mixin graphs in the artifact index ([a8b1868](https://github.com/lensapp/lens-sandbox/commit/a8b1868377f204e551c0b1a96cdfcd115f6c8236))
* relist stopped runs when the service comes back ([48a8dcb](https://github.com/lensapp/lens-sandbox/commit/48a8dcbd22d31da46bfd955088899ee67b9cdbee))
* run a document's pre-start scripts before the workload ([79aa0d7](https://github.com/lensapp/lens-sandbox/commit/79aa0d775e8a2e1ef94486e44499ff4ff1d59595))
* write down what a run launched with, beside its writable layer ([ee71362](https://github.com/lensapp/lens-sandbox/commit/ee71362f53196e09ff6b28ccdd67b1145bdfca20))


### Bug Fixes

* cover what the lifecycle work added, and re-point the e2e cache verbs ([5d3789b](https://github.com/lensapp/lens-sandbox/commit/5d3789b7b9483ed9b7e99148ea1cb72218914915))
* keep an ambiguous run prefix out of rm and inspect arbitration ([7d81573](https://github.com/lensapp/lens-sandbox/commit/7d815733a1f38bf4dd5c25ef36a7652b47b604ef))
* **lns-cli:** answer 125 for a pre-start failure in the sandbox spellings of run and exec ([b16abdb](https://github.com/lensapp/lens-sandbox/commit/b16abdbf952764a8efceda2b3cf4ee7ff7e24dd5))
* **lns-cli:** answer 125 for a run that exits before its session is ready ([8748497](https://github.com/lensapp/lens-sandbox/commit/87484971798496af186b723f71ea50dd9ad2d8b2))
* **lns-cli:** arbitrate bare references at their real home and fail closed ([a43fa38](https://github.com/lensapp/lens-sandbox/commit/a43fa389c6fe198b629043e39ccc8628cdfeedd6))
* **lns-cli:** give ls one spelling — the list alias is removed ([9429e40](https://github.com/lensapp/lens-sandbox/commit/9429e4089f2c8a403b31a437afa97a37ce0bfbf1))
* **lns-cli:** keep both namespaces at 100%, and split the shortcuts' wiring out ([cf2fa1c](https://github.com/lensapp/lens-sandbox/commit/cf2fa1c641d51cac53190ddedebf5b5e7f8381b3))
* **lns-cli:** let the add hint name a spelling the CLI recognizes ([cd21e50](https://github.com/lensapp/lens-sandbox/commit/cd21e50233b9f7d003a1534ec2697d3dc710290b))
* **lns-cli:** let the arbitration ask the cache, not a registry ([238b2a7](https://github.com/lensapp/lens-sandbox/commit/238b2a765d4ac6e72209749f719efeac26ee412c))
* **lns-cli:** let the inspect shortcut carry both namespaces' flags ([2d85582](https://github.com/lensapp/lens-sandbox/commit/2d855821f02f9f71249752f8b2fb1bbfe4555e0e))
* **lns-cli:** let the top-level `lns rm` take -f, as documented ([17337ad](https://github.com/lensapp/lens-sandbox/commit/17337adc43bded0c6dbde3008273b97889a5cbde))
* **lns-cli:** list what a prune would remove before asking ([2e3ebea](https://github.com/lensapp/lens-sandbox/commit/2e3ebea6d817fda80fdc282baca8ecce99e10d44)), closes [#291](https://github.com/lensapp/lens-sandbox/issues/291)
* **lns-cli:** move the prune prompt to stderr where §4.1 says it lives ([de4fc72](https://github.com/lensapp/lens-sandbox/commit/de4fc72f23d41f7ea3c0bf2e63ef645988e31a86))
* **lns-cli:** name the real problem when a pushed mixin is a local directory ([8c3ad9d](https://github.com/lensapp/lens-sandbox/commit/8c3ad9d895b6d3fa40c83b8c97e26c854a2303c7))
* **lns-cli:** refuse a prune with no terminal to ask at, and re-point the e2e cache table ([505c5b7](https://github.com/lensapp/lens-sandbox/commit/505c5b7fb1dfa2ea2e3acbd130cf78261212ef0c))
* **lns-cli:** register the top-level `lns start` shortcut ([75f02a4](https://github.com/lensapp/lens-sandbox/commit/75f02a4a47f7463c829d9103a7fa4bd3a45a1fa5))
* **lns-cli:** resolve a bare cache-verb reference against the Lens hub ([a3d68e4](https://github.com/lensapp/lens-sandbox/commit/a3d68e491dda7c838597d2c0017400f12e46b908))
* **lns-cli:** root a relative --mixin before artifact inspect dispatches it ([6882bd6](https://github.com/lensapp/lens-sandbox/commit/6882bd697677f1e9543211832ab46cd48d068be3))
* **lns-service:** clean up --rm records at rebuild instead of reviving them ([9db6606](https://github.com/lensapp/lens-sandbox/commit/9db66069bc9a1da802814674bffa4ddf733dabf7))
* **lns-service:** make durable deletion a condition of rm's ack ([82dee2f](https://github.com/lensapp/lens-sandbox/commit/82dee2fcef21d2682c3209367910fc3c8cd0b11d))
* **lns-service:** migrate legacy audit chains out of runs/ at service start ([202373f](https://github.com/lensapp/lens-sandbox/commit/202373fca55c72682d065b19163e1972754c406b))
* **lns-service:** protect damaged run records from the orphan sweep ([dfc84dc](https://github.com/lensapp/lens-sandbox/commit/dfc84dcea4e58749eb580c6e8ceb5f7ab8052a67))
* **lns-service:** publish a run's exit only after its VM quiesces ([976b734](https://github.com/lensapp/lens-sandbox/commit/976b734829c2e4a89c07935eb36d338def339343))
* **lns-service:** refuse `start -a` on a running run instead of hanging up ([592a813](https://github.com/lensapp/lens-sandbox/commit/592a813386b7b0f557017f1fec4a41e26afc2929))
* **lns-service:** revalidate prune orphans against the registry before sweeping ([53ef91c](https://github.com/lensapp/lens-sandbox/commit/53ef91cef2b1662eb3682b5c57ced111deadc7ac))
* **lns-session-broker:** keep 126 for a found command whose interpreter is missing ([983ab31](https://github.com/lensapp/lens-sandbox/commit/983ab31050b24bf0930f317ae7518bae9815d666))
* **lns-supervisor:** a pre-start script reads no stdin ([d47a91e](https://github.com/lensapp/lens-sandbox/commit/d47a91e2abe5e488f1fe5d4166137cf1c847de93))
* make the service own the registry login store ([083f740](https://github.com/lensapp/lens-sandbox/commit/083f740e0849833e9c48fc77ccc2fad251c75d46))
* refuse a symlinked README and preflight every README before the first upload ([58dc1c8](https://github.com/lensapp/lens-sandbox/commit/58dc1c8c83db4fe61aaf81582c3d432752fabef8))
* refuse unsafe LNS_HOME roots and show what --purge deletes ([03224e5](https://github.com/lensapp/lens-sandbox/commit/03224e5a608e3709569351dcc734219df6b7f249))

## [0.20.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.19.0...lns-v0.20.0) (2026-08-20)


### Features

* address IPC controls by session ([7278971](https://github.com/lensapp/lens-sandbox/commit/72789715bd45611f2ac6eb161e4dc559a6c15b97))
* route exec session controls independently ([1ac54ac](https://github.com/lensapp/lens-sandbox/commit/1ac54acf6219065105f85cdac89635f7f788e18b))
* support interactive exec sessions ([0381c2a](https://github.com/lensapp/lens-sandbox/commit/0381c2abf0907ce904f790258bf5c0e2221a54e9))


### Bug Fixes

* construct the termination probe before the task can be aborted ([ec28997](https://github.com/lensapp/lens-sandbox/commit/ec289979aed54fc4807c5ef0577b5d542a68eb28))
* **exec:** give confined sessions the workload user's HOME, USER, and cwd ([6a65c4b](https://github.com/lensapp/lens-sandbox/commit/6a65c4b98805f542ae8f9cd5b33015e02d69baa3))
* **exec:** harden the exec identity against hostile passwd data and image env ([a3d09c5](https://github.com/lensapp/lens-sandbox/commit/a3d09c53002c944a70230bdacb0afe9f8e879a54))
* hang up a confined session's child when its host stream vanishes ([6cf9190](https://github.com/lensapp/lens-sandbox/commit/6cf9190f1b89744c25abe7f8e645020647d5c7ec))
* isolate exec client lifecycle ([7a97194](https://github.com/lensapp/lens-sandbox/commit/7a97194bb70e090f51765972e989b5378501910b))
* **lns-cli:** do not lock stdin twice while resolving host binds ([a8c16e0](https://github.com/lensapp/lens-sandbox/commit/a8c16e08aaec6e8537e6305cccddc6c8cc956599))
* **lns-cli:** drop the stdin guard before the session reads the tty ([7047a3b](https://github.com/lensapp/lens-sandbox/commit/7047a3b4fe448a3ea2491ee3fa4e843757610586))
* restore the detach chord for sessions opened without stdin forwarding ([0ce7e20](https://github.com/lensapp/lens-sandbox/commit/0ce7e20ce30a8255c0333641b3f465449a6aa4d4))
* surface cancel-client construction failure instead of a dead Ctrl-C ([17978ec](https://github.com/lensapp/lens-sandbox/commit/17978ec3a0c8ac686891a04c98d11352e35f8737))

## [0.19.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.18.0...lns-v0.19.0) (2026-08-19)


### ⚠ BREAKING CHANGES

* refuse a disk the writer could not format
* decide a mixin's host file per machine too, and share one decision store
* rename the fileset field mountPath to guestPath
* let this machine decide which of its files a pulled sandbox reads
* govern a run by the decisions of the project it runs
* pack a fileset into a layer of the artifact that declares it
* merge what a directory decided into the document its run discloses
* let an lns flag be an lns flag wherever it is written

### Features

* forward per-layer assembly progress from the run into the CLI status line ([b6beba2](https://github.com/lensapp/lens-sandbox/commit/b6beba248aa37ad1c85ae4c0da29c1fabeac6cba))
* give every volume the room it needs to grow later ([20dd3bb](https://github.com/lensapp/lens-sandbox/commit/20dd3bb6002873516a4d94af3e08a61bca2c56e0))
* govern a run by the decisions of the project it runs ([9148369](https://github.com/lensapp/lens-sandbox/commit/914836963c703d43f55dba3ea6217e61694f00a6))
* grow a volume in place, keeping what it holds ([e06829e](https://github.com/lensapp/lens-sandbox/commit/e06829ed984f7b36941af6d7f2903ca236588652))
* let an lns flag be an lns flag wherever it is written ([f9d06e8](https://github.com/lensapp/lens-sandbox/commit/f9d06e88a1ce039b0f13e886d84ef2f6981849a3))
* let this machine decide which of its files a pulled sandbox reads ([0b45931](https://github.com/lensapp/lens-sandbox/commit/0b459316ef52468256530522b20708c8aa9bbd5b))
* merge what a directory decided into the document its run discloses ([80acdec](https://github.com/lensapp/lens-sandbox/commit/80acdec93f4b99111ccbac43ec0565cf34e73e7a))
* pack a fileset into a layer of the artifact that declares it ([d207614](https://github.com/lensapp/lens-sandbox/commit/d2076149c235269555dbea01c66f293488d4876a))
* read the disk size a document asks for ([305ceca](https://github.com/lensapp/lens-sandbox/commit/305ceca4d66d7da5cc791d0819ca4a06dc537903))
* report determinate progress while assembling the rootfs from layers ([97052bb](https://github.com/lensapp/lens-sandbox/commit/97052bb9127bab01bb412ff0c25f475e71b9b4f1))
* say how an entry the run wrote down got into the file ([d368f9d](https://github.com/lensapp/lens-sandbox/commit/d368f9d5dda58b2a93d296bc90ae3b7e24fc3fa8))
* show what a rule says about itself where the run explains the rule ([3c9e278](https://github.com/lensapp/lens-sandbox/commit/3c9e278d5eaa4543ac3272bd51842658afcbf0b0))
* size a named volume from the document that declares it ([f1604d4](https://github.com/lensapp/lens-sandbox/commit/f1604d4ca055c7bcd9df9dc53152d075e47216ba))
* size the run's disk from the document that asked for it ([2d1618b](https://github.com/lensapp/lens-sandbox/commit/2d1618bd4af755ba4c513a8a1bdd711545fc47b4))
* web-based login by default for lns login ([a2e0456](https://github.com/lensapp/lens-sandbox/commit/a2e0456ab3513e66969a3c459d0c3b69fa6fae1a))


### Bug Fixes

* decide a mixin's host file per machine too, and share one decision store ([9defb35](https://github.com/lensapp/lens-sandbox/commit/9defb35df15aeaac8253cbb8e3633aadd33ad98b))
* floor the registry-supplied poll interval at one second ([212f684](https://github.com/lensapp/lens-sandbox/commit/212f68427cb19287bf3dcfb7fc59b37f5a64f399))
* identify device-login requests with the standard lns user agent ([34ceafd](https://github.com/lensapp/lens-sandbox/commit/34ceafd5f6e51f5ce6142464a6f0b673881f669d))
* keep a symlink at the tmp path from redirecting a decision ([bb99d02](https://github.com/lensapp/lens-sandbox/commit/bb99d02c2805e65e0f15d044ac5b55ed5aca335b))
* keep a volume usable after the run that held it was killed ([8aa957e](https://github.com/lensapp/lens-sandbox/commit/8aa957e8992cddc6601d7e0b3829b8c0077df55e))
* let a mixin reference name the document, not only its directory ([aa1e2e9](https://github.com/lensapp/lens-sandbox/commit/aa1e2e942484165407700ca53c2b81ba876e0b91))
* let the approval window appear without taking the keyboard ([cf9b7d0](https://github.com/lensapp/lens-sandbox/commit/cf9b7d0ccbec59fe0982f2454c0658a50df7fe70))
* percent-encode the device code in the token poll form body ([2bdd837](https://github.com/lensapp/lens-sandbox/commit/2bdd837ec49cf349094bd051e07dc018e40400e5))
* probe the service before starting a browser login ([77d096c](https://github.com/lensapp/lens-sandbox/commit/77d096c853db3cd05818db1a72195f54dab69fc0))
* refuse a disk the writer could not format ([ab8282c](https://github.com/lensapp/lens-sandbox/commit/ab8282c84207847e5e6145ae3c1ce69e90e0094d))
* refuse a run before it starts, not after it has ([a73fbcd](https://github.com/lensapp/lens-sandbox/commit/a73fbcdf364cf2a31a532a32919813530ee1428b))
* refuse to hand a non-https verification URL to the platform opener ([a4678ab](https://github.com/lensapp/lens-sandbox/commit/a4678ab488166082f0e97d94ead2738f4525016b))
* reword the browser-login output in plain language and offer the token fallback last ([5617d99](https://github.com/lensapp/lens-sandbox/commit/5617d9967cd3f5cfe42d26d57cc2cc4adb950aca))
* state the disk caching mode Vz must use, instead of taking its default ([9a00e08](https://github.com/lensapp/lens-sandbox/commit/9a00e08a982edfe1690d5d4c680277a4c12276b0)), closes [#247](https://github.com/lensapp/lens-sandbox/issues/247)
* take the tmp with a write that cannot reach its target ([3f06434](https://github.com/lensapp/lens-sandbox/commit/3f064349cdf7ba12a6561790fdba2a4929486188))
* write the connector catalog through the same careful install ([d6457b0](https://github.com/lensapp/lens-sandbox/commit/d6457b0b82d9e58611e42664c8cea5763a407abc))


### Code Refactoring

* rename the fileset field mountPath to guestPath ([a805026](https://github.com/lensapp/lens-sandbox/commit/a805026abc18aa45eac497be2526d947fcd8778e))

## [0.18.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.17.0...lns-v0.18.0) (2026-08-14)


### ⚠ BREAKING CHANGES

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

## [0.17.0](https://github.com/lensapp/lens-sandbox/compare/lns-v0.16.0...lns-v0.17.0) (2026-07-24)


### Features

* seed a declared connector's placeholder env at launch ([b48a7ad](https://github.com/lensapp/lens-sandbox/commit/b48a7ad7c7b6514a783ed1ba12696ab156ee7722))


### Bug Fixes

* arm machine-stored values only for connectors the run consented to ([8f7550a](https://github.com/lensapp/lens-sandbox/commit/8f7550a6b066706f13d48905a215b61e545557d2))
* arm the credential before a connect releases network holds ([0e448b5](https://github.com/lensapp/lens-sandbox/commit/0e448b52567d3231b0870fbcec7ba03b77503958))
* install the armed reconciler before the policy watcher can fire ([3984566](https://github.com/lensapp/lens-sandbox/commit/3984566d55f262a340069222c67d8ef3b4cc7051))
* revoke a disconnected connector's arming on policy reload ([fd75451](https://github.com/lensapp/lens-sandbox/commit/fd75451c943011514927355f5dcfacf9a1f30fa7))
* settle cross-subsystem holds when connector consent lands ([c120fd7](https://github.com/lensapp/lens-sandbox/commit/c120fd76608629a59a01b2f45a6283ebff5929cf))

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
