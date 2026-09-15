#!/bin/sh
set -eu
[ "$#" = 2 ] || { echo 'usage: notarize.sh <app> <zip>' >&2; exit 1; }
: "${NOTARY_PROFILE:?Set NOTARY_PROFILE to the notarytool keychain profile.}"
notary_app=$1
notary_zip=$2
notary_result=$(mktemp)
trap 'rm -f "$notary_result"' EXIT
codesign --verify --deep --strict -R 'anchor apple generic' "$notary_app"
ditto -c -k --sequesterRsrc --keepParent "$notary_app" "$notary_zip"
set -- "$notary_zip" --keychain-profile "$NOTARY_PROFILE" --wait --timeout 15m --output-format json
if [ -n "${NOTARY_KEYCHAIN:-}" ]; then set -- "$@" --keychain "$NOTARY_KEYCHAIN"; fi
xcrun notarytool submit "$@" > "$notary_result"
[ "$(plutil -extract status raw -o - "$notary_result")" = Accepted ] || {
    cat "$notary_result" >&2
    echo 'Apple did not accept the app for notarization.' >&2
    exit 1
}
xcrun stapler staple "$notary_app"
xcrun stapler validate "$notary_app"
spctl --assess --type execute --verbose=2 "$notary_app"
ditto -c -k --sequesterRsrc --keepParent "$notary_app" "$notary_zip"
