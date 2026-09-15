#!/bin/sh
set -eu
installer_test_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
installer_test_tmp=$(mktemp -d)
trap 'rm -rf "$installer_test_tmp"' EXIT HUP INT TERM
mkdir -p "$installer_test_tmp/bin" "$installer_test_tmp/helpers" "$installer_test_tmp/LNS.app/Contents/Resources" "$installer_test_tmp/home"
printf '#!/bin/sh\necho "lns 0.25.0"\n' > "$installer_test_tmp/helpers/lns"
cp "$installer_test_tmp/helpers/lns" "$installer_test_tmp/helpers/lns-service"
chmod +x "$installer_test_tmp/helpers/"*
cat > "$installer_test_tmp/LNS.app/Contents/Resources/install.sh" <<'MOCK'
#!/bin/sh
set -eu
printf native > "$INSTALL_DIR/result"
MOCK
cat > "$installer_test_tmp/bin/uname" <<'MOCK'
#!/bin/sh
case "$1" in -s) echo Darwin;; -m) echo arm64;; -r) echo 26;; esac
MOCK
cat > "$installer_test_tmp/bin/curl" <<'MOCK'
#!/bin/sh
set -eu
destination=''
while [ "$#" -gt 0 ]; do
  case "$1" in -A) shift 2;; -o) destination=$2; shift 2;; -*) shift;; *) url=$1; shift;; esac
done
case "$url" in
  */lns-latest.json) printf '{"version":"0.25.0","platforms":{"darwin-aarch64":{"url":"https://get.lns.run/lns-0.25.0-darwin-aarch64.%s","sha256":"digest"}}}' "$INSTALLER_TEST_EXTENSION";;
  *.sha256) printf 'digest  archive\n' > "$destination";;
  *.tar.gz)
    [ "$INSTALLER_TEST_EXTENSION" = tar.gz ] || exit 1
    tar czf "$destination" -C "$INSTALLER_TEST_SOURCE/helpers" lns lns-service;;
  *.zip)
    [ "$INSTALLER_TEST_EXTENSION" = zip ] || { echo 'FAIL: installer requested a ZIP before the manifest published one' >&2; exit 1; }
    touch "$destination";;
  *) exit 1;;
esac
MOCK
cat > "$installer_test_tmp/bin/ditto" <<'MOCK'
#!/bin/sh
set -eu
mkdir -p "$4"
cp -R "$INSTALLER_TEST_SOURCE/LNS.app" "$4/LNS.app"
MOCK
cat > "$installer_test_tmp/bin/codesign" <<'MOCK'
#!/bin/sh
[ "${INSTALLER_TEST_BAD_SIGNATURE:-0}" != 1 ]
MOCK
printf '#!/bin/sh\nexit 0\n' > "$installer_test_tmp/bin/spctl"
printf '#!/bin/sh\necho "digest  archive"\n' > "$installer_test_tmp/bin/sha256sum"
cat > "$installer_test_tmp/bin/plutil" <<'MOCK'
#!/usr/bin/env python3
import json,sys
assert sys.argv[1:] == ['-extract', 'platforms.darwin-aarch64.url', 'raw', '-o', '-', '-']
print(json.load(sys.stdin)['platforms']['darwin-aarch64']['url'])
MOCK
chmod +x "$installer_test_tmp/bin/"*
export PATH="$installer_test_tmp/bin:$PATH" INSTALLER_TEST_SOURCE="$installer_test_tmp"
export INSTALL_DIR="$installer_test_tmp/install" APP_DIR="$installer_test_tmp/apps" LNS_NO_SERVICE=1
mkdir -p "$INSTALL_DIR"
INSTALLER_TEST_EXTENSION=tar.gz env HOME="$installer_test_tmp/home" TMPDIR="$installer_test_tmp" bash "$installer_test_root/scripts/lns-install/lns-install.sh"
[ -x "$INSTALL_DIR/lns" ] && [ ! -e "$INSTALL_DIR/result" ]
echo 'PASS: the installer follows the current tarball manifest before native publication'
INSTALLER_TEST_EXTENSION=zip env HOME="$installer_test_tmp/home" TMPDIR="$installer_test_tmp" bash "$installer_test_root/scripts/lns-install/lns-install.sh"
[ "$(cat "$INSTALL_DIR/result")" = native ]
echo 'PASS: a native manifest selects the verified complete-app installer'
rm "$INSTALL_DIR/result"
if INSTALLER_TEST_EXTENSION=zip INSTALLER_TEST_BAD_SIGNATURE=1 env HOME="$installer_test_tmp/home" TMPDIR="$installer_test_tmp" bash "$installer_test_root/scripts/lns-install/lns-install.sh"; then
  echo 'FAIL: unsigned native app was installed' >&2; exit 1
fi
[ ! -e "$INSTALL_DIR/result" ]
echo 'PASS: native signature failure cannot fall back to installing loose helpers'
