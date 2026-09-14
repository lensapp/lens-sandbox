#!/bin/sh
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM
export WORK
export REAL_JQ
REAL_JQ=$(command -v jq)
mkdir -p "$WORK/bin" "$WORK/home"
export LNS_HOME="$WORK/home"
export LNS_ACTION_REGISTRY=example.invalid
export LNS_ACTION_USERNAME=dummy
export LNS_ACTION_PASSWORD=dummy-data
cat >"$WORK/bin/lns" <<'STUB'
#!/bin/sh
cat >/dev/null
echo 'background service must be running'
exit 1
STUB
cat >"$WORK/bin/jq" <<'STUB'
#!/bin/sh
case "$*" in
    '-e . '*) exec "$REAL_JQ" "$@" ;;
esac
find "$LNS_HOME" -name 'registry-auth.json.*' -type f -perm 600 >"$WORK/secure-temp"
if [ "${FAIL_WRITE:-false}" = true ]; then
    printf 'partial'
    exit 8
fi
exec "$REAL_JQ" "$@"
STUB
cat >"$WORK/bin/mv" <<'STUB'
#!/bin/sh
if [ "${FAIL_MOVE:-false}" = true ]; then exit 9; fi
exec /bin/mv "$@"
STUB
chmod +x "$WORK/bin/"*
export PATH="$WORK/bin:$PATH"
FAIL=0
PASS=0
check() {
    name=$1
    shift
    if "$@"; then
        echo "ok: $name"
        PASS=$((PASS + 1))
    else
        echo "FAIL: $name"
        FAIL=$((FAIL + 1))
    fi
}

printf 'untouched' >"$WORK/target"
export AUTH_SCRIPT="$ROOT/actions/lib/registry-auth.sh"
(umask 000; bash -c '
    ln -s "$WORK/target" "$LNS_HOME/registry-auth.json.$$.tmp"
    exec bash "$AUTH_SCRIPT" add
') >"$WORK/log" 2>&1
check 'predictable symlink target stays unchanged' test "$(cat "$WORK/target")" = untouched
check 'predictable symlink is not substituted into auth file' test ! -L "$LNS_HOME/registry-auth.json"
check 'temporary file is 0600 before jq writes' test -s "$WORK/secure-temp"
check 'installed auth file is 0600' test -n "$(find "$LNS_HOME/registry-auth.json" -type f -perm 600)"
check 'dummy credential is installed' "$REAL_JQ" -e '."example.invalid".secret == "dummy-data"' "$LNS_HOME/registry-auth.json"
find "$LNS_HOME" -name 'registry-auth.json.*' -type l -delete
cp "$LNS_HOME/registry-auth.json" "$WORK/before"
for failure in FAIL_WRITE FAIL_MOVE; do
    set +e
    env "$failure=true" bash "$AUTH_SCRIPT" add >"$WORK/log" 2>&1
    status=$?
    set -e
    check "$failure: fails" test "$status" -ne 0
    check "$failure: preserves original auth file" cmp -s "$WORK/before" "$LNS_HOME/registry-auth.json"
    check "$failure: removes temporary file" test -z "$(find "$LNS_HOME" -name 'registry-auth.json.*')"
done
bash "$AUTH_SCRIPT" remove </dev/null >"$WORK/log" 2>&1
check 'logout retains secure mode' test -n "$(find "$LNS_HOME/registry-auth.json" -type f -perm 600)"
check 'logout removes dummy credential' "$REAL_JQ" -e 'length == 0' "$LNS_HOME/registry-auth.json"
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
