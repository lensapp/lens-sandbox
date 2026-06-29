# Changelog

## [0.6.0](https://github.com/lensapp/lens-sandbox/compare/lns-install-v0.5.0...lns-install-v0.6.0) (2026-06-29)


### Features

* **vm:** resolve cloud-hypervisor/virtiofsd from PATH on Linux ([8dd4795](https://github.com/lensapp/lens-sandbox/commit/8dd479516c643700d96e4e4bda51fa98f17a2671))

## [0.5.0](https://github.com/lensapp/lens-sandbox/compare/lns-install-v0.4.0...lns-install-v0.5.0) (2026-06-05)


### Features

* auto-start the service on install and login (lns service enable/disable) ([7196a95](https://github.com/lensapp/lens-sandbox/commit/7196a95a9f7a6126aa756907299299d504aba996))


### Bug Fixes

* never spawn a competing service instance during enable ([0a9758f](https://github.com/lensapp/lens-sandbox/commit/0a9758f89b94527f925d4cf69c9ba3078c5294d9))

## [0.4.0](https://github.com/lensapp/lens-sandbox/compare/lns-install-v0.3.0...lns-install-v0.4.0) (2026-05-21)


### Features

* **lns-install:** extract and install lns-service alongside lns (LNS-5) ([d55dd01](https://github.com/lensapp/lens-sandbox/commit/d55dd0184feaa088b485735f1ddfbe6276aa6863))
* **lns-install:** warn on missing tray runtime libs on Linux ([b7b0b6b](https://github.com/lensapp/lens-sandbox/commit/b7b0b6b20a75683f5f2f6eb1d5cea48192d60625))

## [0.3.0](https://github.com/lensapp/lens-sandbox/compare/lns-install-v0.2.0...lns-install-v0.3.0) (2026-05-15)


### Features

* **lns-install:** identify install script in CDN access logs ([0a4364e](https://github.com/lensapp/lens-sandbox/commit/0a4364ec464f8925785231010b6fad1ab56836e2))

## [0.2.0](https://github.com/lensapp/lens-sandbox/compare/lns-install-v0.1.0...lns-install-v0.2.0) (2026-05-15)


### Features

* lns-cli and lns-init crates with build + release CI ([213318c](https://github.com/lensapp/lens-sandbox/commit/213318c31a7595aeebc50d60d566139a8056c84a))


### Bug Fixes

* **install:** harden lns-install.sh against silent failures ([cec502c](https://github.com/lensapp/lens-sandbox/commit/cec502cadee5517582a048d517992fa3608fbbec))
