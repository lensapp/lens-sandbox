SWIFT ?= swift
SIGN_IDENTITY ?= -
VERSION ?= $(shell sed -n 's/^version = "\([^"]*\)"/\1/p' ../../crates/lns-cli/Cargo.toml | head -1)
BUILD_NUMBER ?= $(shell git rev-list --count HEAD)
HELPER_DIR ?= ../../bin

.PHONY: test build verify package shell-tests smoke

test: shell-tests
	$(SWIFT) test

shell-tests:
	sh scripts/bundle.test.sh

build:
	@test "$$(uname -s)" = Darwin || { echo 'Building LNS.app requires macOS and Xcode.' >&2; exit 1; }
	$(SWIFT) build -c release --product LNS
	SIGN_IDENTITY="$(SIGN_IDENTITY)" sh scripts/bundle.sh .build/release/LNS dist "$(VERSION)" "$(BUILD_NUMBER)"

package:
	@test "$$(uname -s)" = Darwin || { echo 'Packaging LNS.app requires macOS and Xcode.' >&2; exit 1; }
	$(MAKE) -C ../.. build CARGO_LOCKED=--locked
	$(SWIFT) build -c release --product LNS
	SIGN_IDENTITY="$(SIGN_IDENTITY)" sh scripts/bundle.sh .build/release/LNS dist "$(VERSION)" "$(BUILD_NUMBER)" "$(HELPER_DIR)"
	ditto -c -k --sequesterRsrc --keepParent dist/LNS.app dist/LNS-macos.zip

smoke:
	$(SWIFT) build --product LNSClientSmoke
	python3 Smoke/run.py dist/LNS.app/Contents/Helpers/lns-service .build/debug/LNSClientSmoke

verify: test build
