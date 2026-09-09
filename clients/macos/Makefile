SWIFT ?= swift
SIGN_IDENTITY ?= -

.PHONY: test build verify

test:
	$(SWIFT) test

build:
	@test "$$(uname -s)" = Darwin || { echo 'Building LNS.app requires macOS and Xcode.' >&2; exit 1; }
	$(SWIFT) build -c release --product LNS
	mkdir -p dist/LNS.app/Contents/MacOS
	cp Info.plist dist/LNS.app/Contents/Info.plist
	install -m 755 .build/release/LNS dist/LNS.app/Contents/MacOS/LNS
	codesign --force --sign "$(SIGN_IDENTITY)" dist/LNS.app
	codesign --verify --strict dist/LNS.app

verify: test build
