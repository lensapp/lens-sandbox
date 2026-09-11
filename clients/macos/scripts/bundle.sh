#!/bin/sh
set -eu
if [ "$#" -lt 4 ] || [ "$#" -gt 5 ]; then
    echo 'usage: bundle.sh <executable> <output-directory> <version> <build-number> [helpers-directory]' >&2
    exit 1
fi
bundle_scripts=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
bundle_source=$1
bundle_output=$2
bundle_version=$3
bundle_number=$4
bundle_helpers=${5:-}
bundle_identity=${SIGN_IDENTITY:--}
printf '%s\n' "$bundle_version" | LC_ALL=C awk '/^[0-9]+\.[0-9]+\.[0-9]+$/ { valid=1 } END { exit !valid }' || {
    echo 'Bundle version must contain three numeric components.' >&2; exit 1;
}
case "$bundle_number" in ''|*[!0-9]*) echo 'Bundle build number must be numeric.' >&2; exit 1;; esac
test -x "$bundle_source" || { echo 'Missing app executable.' >&2; exit 1; }
if [ -n "$bundle_helpers" ]; then
    test -x "$bundle_helpers/lns" && test -x "$bundle_helpers/lns-service" || {
        echo 'Both lns and lns-service helpers are required.' >&2; exit 1;
    }
    test "$("$bundle_helpers/lns" --version)" = "lns $bundle_version" || {
        echo 'The CLI helper version does not match the app.' >&2; exit 1;
    }
fi
mkdir -p "$bundle_output"
bundle_output=$(CDPATH= cd -- "$bundle_output" && pwd)
test ! -L "$bundle_output/LNS.app" || { echo 'Refusing to replace a symlinked app.' >&2; exit 1; }
bundle_stage=$(mktemp -d "$bundle_output/.lns-build.XXXXXX")
trap 'rm -rf "$bundle_stage"' EXIT HUP INT TERM
bundle_app="$bundle_stage/LNS.app"
mkdir -p "$bundle_app/Contents/MacOS"
mkdir -p "$bundle_app/Contents/Resources"
install -m 644 "$bundle_scripts/../../../crates/lns-service/assets/lnsTemplate@2x.png" "$bundle_app/Contents/Resources/lnsTemplate@2x.png"
install -m 644 "$bundle_scripts/../Assets/LNS.icns" "$bundle_app/Contents/Resources/LNS.icns"
install -m 755 "$bundle_source" "$bundle_app/Contents/MacOS/LNS"
sed -e "s/@VERSION@/$bundle_version/g" -e "s/@BUILD_NUMBER@/$bundle_number/g" \
    "$bundle_scripts/../Info.plist" > "$bundle_app/Contents/Info.plist"

sign_binary() {
    if [ "$bundle_identity" = - ]; then
        codesign --force --sign "$bundle_identity" "$@"
    else
        codesign --force --options runtime --timestamp --sign "$bundle_identity" "$@"
    fi
}

if [ -n "$bundle_helpers" ]; then
    mkdir -p "$bundle_app/Contents/Helpers"
    for bundle_helper in lns lns-service; do
        install -m 755 "$bundle_helpers/$bundle_helper" "$bundle_app/Contents/Helpers/$bundle_helper"
        sign_binary --entitlements "$bundle_scripts/../../../crates/lns-cli/lns.entitlements" "$bundle_app/Contents/Helpers/$bundle_helper"
        codesign --verify --strict "$bundle_app/Contents/Helpers/$bundle_helper"
    done
fi
sign_binary "$bundle_app"
codesign --verify --deep --strict "$bundle_app"
bundle_backup=''
if [ -e "$bundle_output/LNS.app" ]; then
    bundle_backup=$(mktemp -d "$bundle_output/.lns-previous.XXXXXX")
    mv "$bundle_output/LNS.app" "$bundle_backup/LNS.app"
fi
if ! mv "$bundle_app" "$bundle_output/LNS.app"; then
    if [ -n "$bundle_backup" ]; then mv "$bundle_backup/LNS.app" "$bundle_output/LNS.app"; fi
    exit 1
fi
printf 'Built %s\n' "$bundle_output/LNS.app"
if [ -n "$bundle_backup" ]; then printf 'Previous bundle retained at %s/LNS.app\n' "$bundle_backup"; fi
