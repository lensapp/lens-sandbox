#!/bin/sh
set -eu
[ "$#" = 2 ] || { echo 'usage: ci-release.sh <helpers-directory> <version>' >&2; exit 1; }
: "${MACOS_CERTIFICATE_P12:?Missing MACOS_CERTIFICATE_P12 release secret}"
: "${MACOS_CERTIFICATE_PASSWORD:?Missing MACOS_CERTIFICATE_PASSWORD release secret}"
: "${SIGN_IDENTITY:?Missing MACOS_SIGN_IDENTITY release secret}"
: "${MACOS_NOTARY_KEY_P8:?Missing MACOS_NOTARY_KEY_P8 release secret}"
: "${MACOS_NOTARY_KEY_ID:?Missing MACOS_NOTARY_KEY_ID release secret}"
: "${MACOS_NOTARY_ISSUER_ID:?Missing MACOS_NOTARY_ISSUER_ID release secret}"
[ "$SIGN_IDENTITY" != - ] || { echo 'Public releases require Developer ID signing.' >&2; exit 1; }
release_helpers=$(CDPATH= cd -- "$1" && pwd)
release_version=$2
release_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
release_temp=$(mktemp -d)
export SIGN_KEYCHAIN="$release_temp/signing.keychain-db"
export NOTARY_KEYCHAIN="$SIGN_KEYCHAIN"
export NOTARY_PROFILE=lns-release
cleanup_release() {
    security delete-keychain "$SIGN_KEYCHAIN" >/dev/null 2>&1 || true
    rm -rf "$release_temp"
}
trap cleanup_release EXIT
trap 'exit 1' HUP INT TERM
release_password=$(openssl rand -hex 24)
printf '%s' "$MACOS_CERTIFICATE_P12" | base64 --decode > "$release_temp/signing.p12"
printf '%s' "$MACOS_NOTARY_KEY_P8" > "$release_temp/notary.p8"
security create-keychain -p "$release_password" "$SIGN_KEYCHAIN"
security set-keychain-settings -lut 21600 "$SIGN_KEYCHAIN"
security unlock-keychain -p "$release_password" "$SIGN_KEYCHAIN"
security import "$release_temp/signing.p12" -P "$MACOS_CERTIFICATE_PASSWORD" -k "$SIGN_KEYCHAIN" -T /usr/bin/codesign >/dev/null
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$release_password" "$SIGN_KEYCHAIN" >/dev/null
xcrun notarytool store-credentials "$NOTARY_PROFILE" --key "$release_temp/notary.p8" --key-id "$MACOS_NOTARY_KEY_ID" --issuer "$MACOS_NOTARY_ISSUER_ID" --keychain "$NOTARY_KEYCHAIN" >/dev/null
swift build --package-path "$release_root" -c release --product LNS
sh "$release_root/scripts/runtime.test.sh"
sh "$release_root/scripts/bundle.sh" "$release_root/.build/release/LNS" "$release_root/dist" "$release_version" "${GITHUB_RUN_NUMBER:?Missing release build number}" "$release_helpers"
sh "$release_root/scripts/notarize.sh" "$release_root/dist/LNS.app" "$release_root/dist/LNS-macos.zip"
make -C "$release_root" smoke
