#!/bin/sh
set -eu
signing_test_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
signing_test_tmp=$(mktemp -d)
trap 'rm -rf "$signing_test_tmp"' EXIT HUP INT TERM
printf "team='@MACOS_TEAM_ID@'\n" > "$signing_test_tmp/template"
MACOS_TEAM_ID=TESTTEAM01 python3 "$signing_test_root/scripts/configure-macos-signing.py" "$signing_test_tmp/template"
[ "$(cat "$signing_test_tmp/template")" = "team='TESTTEAM01'" ] || { echo 'FAIL: release signing team was not embedded' >&2; exit 1; }
echo 'PASS: release signing team is embedded'
for signing_test_team in '' invalid 'ABCDEFGHIJK' 'TESTTEAM0"'; do
  printf "team='@MACOS_TEAM_ID@'\n" > "$signing_test_tmp/template"
  if MACOS_TEAM_ID="$signing_test_team" python3 "$signing_test_root/scripts/configure-macos-signing.py" "$signing_test_tmp/template"; then
    echo 'FAIL: missing or invalid signing team accepted' >&2; exit 1
  fi
  [ "$(cat "$signing_test_tmp/template")" = "team='@MACOS_TEAM_ID@'" ]
done
echo 'PASS: invalid configuration cannot modify the installer'
printf 'no placeholder\n' > "$signing_test_tmp/invalid"
if MACOS_TEAM_ID=TESTTEAM01 python3 "$signing_test_root/scripts/configure-macos-signing.py" "$signing_test_tmp/template" "$signing_test_tmp/invalid"; then
  echo 'FAIL: a release installer without its team placeholder was accepted' >&2; exit 1
fi
[ "$(cat "$signing_test_tmp/template")" = "team='@MACOS_TEAM_ID@'" ]
echo 'PASS: all templates are validated before configuring any'
echo "Results: 3 passed, 0 failed"
