#!/bin/sh
set -eu
install_test_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
install_test_tmp=$(mktemp -d)
trap 'rm -rf "$install_test_tmp"' EXIT HUP INT TERM
mkdir -p "$install_test_tmp/home" "$install_test_tmp/bin" "$install_test_tmp/source/LNS.app/Contents/Helpers" "$install_test_tmp/apps/LNS.app/Contents/Helpers"
printf old > "$install_test_tmp/apps/LNS.app/marker"
printf new > "$install_test_tmp/source/LNS.app/marker"
cat > "$install_test_tmp/source/LNS.app/Contents/Helpers/lns" <<'HELPER'
#!/bin/sh
printf '%s\n' "$*" >> "$INSTALL_TEST_LOG"
case "$*" in
  --version) echo 'lns 0.25.0';;
  'service status --format json')
    if [ "${INSTALL_TEST_INVALID_STATUS:-0}" = 1 ]; then echo '{}';
    else printf '{"running":%s}\n' "${INSTALL_TEST_RUNNING:-false}"; fi;;
  'service stop') test "${INSTALL_TEST_STOP_FAIL:-0}" != 1;;
  'service start') test "${INSTALL_TEST_START_FAIL:-0}" != 1;;
esac
HELPER
cp "$install_test_tmp/source/LNS.app/Contents/Helpers/lns" "$install_test_tmp/apps/LNS.app/Contents/Helpers/lns"
cp "$install_test_tmp/source/LNS.app/Contents/Helpers/lns" "$install_test_tmp/source/LNS.app/Contents/Helpers/lns-service"
chmod +x "$install_test_tmp/source/LNS.app/Contents/Helpers/"* "$install_test_tmp/apps/LNS.app/Contents/Helpers/lns"
for install_test_command in codesign spctl osascript defaults; do
  cat > "$install_test_tmp/bin/$install_test_command" <<'COMMAND'
#!/bin/sh
printf '%s %s\n' "$(basename "$0")" "$*" >> "$INSTALL_TEST_LOG"
if [ "$(basename "$0")" = codesign ] && [ "${INSTALL_TEST_BAD_SIGNATURE:-0}" = 1 ]; then exit 1; fi
COMMAND
  chmod +x "$install_test_tmp/bin/$install_test_command"
done
cat > "$install_test_tmp/bin/ditto" <<'COMMAND'
#!/bin/sh
cp -R "$1" "$2"
COMMAND
chmod +x "$install_test_tmp/bin/ditto"
export PATH="$install_test_tmp/bin:$PATH"
export INSTALL_TEST_LOG="$install_test_tmp/actions"
export LNS_NO_SERVICE=1
env HOME="$install_test_tmp/home" sh "$install_test_root/install.sh" "$install_test_tmp/source/LNS.app" "$install_test_tmp/apps/LNS.app" "$install_test_tmp/links" 0.25.0
[ "$(cat "$install_test_tmp/apps/LNS.app/marker")" = new ]
[ "$(readlink "$install_test_tmp/links/lns")" = "$install_test_tmp/apps/LNS.app/Contents/Helpers/lns" ]
[ "$(readlink "$install_test_tmp/links/lns-service")" = "$install_test_tmp/apps/LNS.app/Contents/Helpers/lns-service" ]
printf '%s\n' 'PASS: the complete app and CLI links are installed together'
printf preserved > "$install_test_tmp/apps/LNS.app/marker"
if INSTALL_TEST_BAD_SIGNATURE=1 env HOME="$install_test_tmp/home" sh "$install_test_root/install.sh" "$install_test_tmp/source/LNS.app" "$install_test_tmp/apps/LNS.app" "$install_test_tmp/links" 0.25.0; then
  echo 'FAIL: invalid signature was installed' >&2; exit 1
fi
[ "$(cat "$install_test_tmp/apps/LNS.app/marker")" = preserved ]
printf '%s\n' 'PASS: signature failure leaves the installed app untouched'

if LNS_NO_SERVICE=0 INSTALL_TEST_START_FAIL=1 env HOME="$install_test_tmp/home" sh "$install_test_root/install.sh" "$install_test_tmp/source/LNS.app" "$install_test_tmp/apps/LNS.app" "$install_test_tmp/links" 0.25.0; then
  echo 'FAIL: startup failure was accepted' >&2; exit 1
fi
[ "$(cat "$install_test_tmp/apps/LNS.app/marker")" = preserved ]
[ "$(readlink "$install_test_tmp/links/lns")" = "$install_test_tmp/apps/LNS.app/Contents/Helpers/lns" ]
printf '%s\n' 'PASS: startup failure restores the app and CLI links'
if [ "$(tail -n 1 "$INSTALL_TEST_LOG")" != 'service stop' ]; then
  echo 'FAIL: rollback did not stop the replacement service' >&2; exit 1
fi
if INSTALL_TEST_RUNNING=true INSTALL_TEST_STOP_FAIL=1 env HOME="$install_test_tmp/home" sh "$install_test_root/install.sh" "$install_test_tmp/source/LNS.app" "$install_test_tmp/apps/LNS.app" "$install_test_tmp/links" 0.25.0; then
  echo 'FAIL: an active service was replaced after stop failed' >&2; exit 1
fi
[ "$(cat "$install_test_tmp/apps/LNS.app/marker")" = preserved ]
printf '%s\n' 'PASS: failed shutdown cannot replace the app'

if INSTALL_TEST_INVALID_STATUS=1 env HOME="$install_test_tmp/home" sh "$install_test_root/install.sh" "$install_test_tmp/source/LNS.app" "$install_test_tmp/apps/LNS.app" "$install_test_tmp/links" 0.25.0; then
  echo 'FAIL: missing service state was treated as stopped' >&2; exit 1
fi
[ "$(cat "$install_test_tmp/apps/LNS.app/marker")" = preserved ]
printf '%s\n' 'PASS: unknown service state prevents replacement'
