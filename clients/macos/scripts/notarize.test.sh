#!/bin/sh
set -eu
notary_scripts=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
notary_tmp=$(mktemp -d)
trap 'rm -rf "$notary_tmp"' EXIT
mkdir -p "$notary_tmp/bin" "$notary_tmp/LNS.app"
export NOTARY_TEST_LOG="$notary_tmp/log"
export NOTARY_PROFILE=test-profile
export PATH="$notary_tmp/bin:$PATH"
for notary_command in codesign spctl ditto; do
    cat > "$notary_tmp/bin/$notary_command" <<'COMMAND'
#!/bin/sh
printf '%s %s\n' "$(basename "$0")" "$*" >> "$NOTARY_TEST_LOG"
COMMAND
    chmod +x "$notary_tmp/bin/$notary_command"
done
cat > "$notary_tmp/bin/xcrun" <<'COMMAND'
#!/bin/sh
printf 'xcrun %s\n' "$*" >> "$NOTARY_TEST_LOG"
case "$*" in
  'notarytool submit '*) printf '{"status":"%s"}\n' "${NOTARY_TEST_STATUS:-Accepted}";;
  'stapler staple '*) test "${NOTARY_TEST_STAPLE_FAIL:-0}" != 1;;
esac
COMMAND
cat > "$notary_tmp/bin/plutil" <<'COMMAND'
#!/bin/sh
for item do notary_result=$item; done
sed -n 's/.*"status":"\([^"]*\)".*/\1/p' "$notary_result"
COMMAND
chmod +x "$notary_tmp/bin/xcrun" "$notary_tmp/bin/plutil"
sh "$notary_scripts/notarize.sh" "$notary_tmp/LNS.app" "$notary_tmp/LNS.zip"
[ "$(tail -1 "$NOTARY_TEST_LOG" | cut -d' ' -f1)" = ditto ] || { echo 'FAIL: archive was not rebuilt after stapling' >&2; exit 1; }
grep -q 'xcrun stapler validate' "$NOTARY_TEST_LOG"
printf '%s\n' 'PASS: accepted notarization is stapled, validated, and re-archived'
: > "$NOTARY_TEST_LOG"
if NOTARY_TEST_STATUS=Invalid sh "$notary_scripts/notarize.sh" "$notary_tmp/LNS.app" "$notary_tmp/LNS.zip"; then
    echo 'FAIL: rejected notarization was accepted' >&2; exit 1
fi
if grep -q 'stapler' "$NOTARY_TEST_LOG"; then echo 'FAIL: a rejected app was stapled' >&2; exit 1; fi
printf '%s\n' 'PASS: rejected notarization cannot reach distribution'
if NOTARY_TEST_STAPLE_FAIL=1 sh "$notary_scripts/notarize.sh" "$notary_tmp/LNS.app" "$notary_tmp/LNS.zip"; then
    echo 'FAIL: failed stapling was accepted' >&2; exit 1
fi
printf '%s\n' 'PASS: stapling failure stops distribution'
