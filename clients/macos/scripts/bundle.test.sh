#!/bin/sh
set -eu
bundle_scripts=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
bundle_test_root=$(mktemp -d)
trap 'rm -rf "$bundle_test_root"' EXIT HUP INT TERM
export PATH="$bundle_scripts/fixtures:$PATH"
export BUNDLE_SIGN_LOG="$bundle_test_root/sign.log"
mkdir -p "$bundle_test_root/helpers" "$bundle_test_root/output"
cp "$bundle_scripts/fixtures/lns" "$bundle_test_root/helpers/lns"
cp "$bundle_scripts/fixtures/lns" "$bundle_test_root/helpers/lns-service"
chmod +x "$bundle_test_root/helpers/lns" "$bundle_test_root/helpers/lns-service"

build_bundle() {
    sh "$bundle_scripts/bundle.sh" "$bundle_scripts/fixtures/lns" "$bundle_test_root/output" 0.25.0 42 "$bundle_test_root/helpers"
}

build_bundle
test -x "$bundle_test_root/output/LNS.app/Contents/MacOS/LNS" || { echo 'FAIL: no launchable LNS app was packaged' >&2; exit 1; }
test -x "$bundle_test_root/output/LNS.app/Contents/Helpers/lns"
test -x "$bundle_test_root/output/LNS.app/Contents/Helpers/lns-service"
grep -q '<string>0.25.0</string>' "$bundle_test_root/output/LNS.app/Contents/Info.plist"
grep -q '<string>42</string>' "$bundle_test_root/output/LNS.app/Contents/Info.plist"
grep -q -- '--entitlements' "$BUNDLE_SIGN_LOG"
grep -q -- '--verify --strict' "$BUNDLE_SIGN_LOG"
echo 'PASS: versioned bundle contains executable helpers and verified signatures'

cp "$bundle_test_root/output/LNS.app/Contents/Info.plist" "$bundle_test_root/original.plist"
if BUNDLE_SIGN_FAIL=1 build_bundle; then echo 'FAIL: signing failure was accepted' >&2; exit 1; fi
cmp "$bundle_test_root/original.plist" "$bundle_test_root/output/LNS.app/Contents/Info.plist"
echo 'PASS: failed signing preserves the previous bundle'

build_bundle
test "$(find "$bundle_test_root/output" -path '*/Contents/Info.plist' | wc -l | tr -d ' ')" = 2
echo 'PASS: replacement keeps a recoverable previous bundle'

if sh "$bundle_scripts/bundle.sh" "$bundle_scripts/fixtures/lns" "$bundle_test_root/output" '<bad>' 42; then
    echo 'FAIL: unsafe version reached the property list' >&2; exit 1
fi
if sh "$bundle_scripts/bundle.sh" "$bundle_scripts/fixtures/lns" "$bundle_test_root/output" 9.9.9 42 "$bundle_test_root/helpers"; then
    echo 'FAIL: mismatched CLI version was packaged' >&2; exit 1
fi
echo 'PASS: invalid metadata and mismatched helpers are refused'
