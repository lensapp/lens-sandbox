#!/bin/sh
# Runs the already-built test binaries a second time under a deliberately
# different ambient environment. A test whose result depends on the host —
# uid, umask, TZ, locale, HOME, TMPDIR, proxy — passes the first pass and
# fails this one.
#
# USAGE: env-parity.sh <binary> [<binary>…]
#
# The caller enumerates the binaries (the Makefile uses `cargo test --no-run
# --message-format=json`). Each is run with empty argv: libtest and the
# cucumber harnesses both name their own failures, and neither accepts a
# shared argv.
#
# Needs `setpriv` (util-linux) to drop privileges when the first pass ran as
# root. Without it the pass still runs, with the environment perturbed but
# the uid unchanged.
#
# Set LNS_ENV_PARITY=0 to skip.

set -eu

[ "${LNS_ENV_PARITY:-1}" = "0" ] && { echo "env-parity: skipped (LNS_ENV_PARITY=0)"; exit 0; }
[ "$#" -gt 0 ] || { echo "env-parity: no test binaries given" >&2; exit 2; }

widened=""
scratch=$(mktemp -d /tmp/lnsP.XXXX)

restore() {
    for dir in $widened; do
        chmod o-x "$dir" 2>/dev/null || true
    done
    rm -rf "$scratch"
}
trap restore EXIT
trap 'restore; exit 130' INT TERM

# `out` stays in the scratch root, which the unprivileged pass cannot write,
# so it cannot be pre-empted by a symlink.
mkdir -p "$scratch/home" "$scratch/tmp" "$scratch/profraw"
out="$scratch/out"

# The unprivileged pass reads the binaries in place, and the cucumber
# harnesses reopen their .feature files by absolute path, so every ancestor
# of the workspace needs traverse permission. Grant o+x only — never o+r —
# and take it back on the way out.
grant_traversal() {
    dir=$1
    while [ "$dir" != "/" ] && [ "$dir" != "." ]; do
        case "$(ls -ld "$dir" 2>/dev/null | cut -c10)" in
            x | t) : ;;
            *)
                if chmod o+x "$dir" 2>/dev/null; then
                    widened="$widened $dir"
                fi
                ;;
        esac
        dir=$(dirname "$dir")
    done
}

as_unprivileged=""
if [ "$(id -u)" = "0" ] && command -v setpriv >/dev/null 2>&1; then
    # The workspace for the .feature files the cucumber harnesses reopen, and
    # each binary's own directory, which a custom CARGO_TARGET_DIR can put
    # somewhere else entirely.
    grant_traversal "$(cd "$(dirname "$0")/.." && pwd)"
    for bin in "$@"; do
        grant_traversal "$(dirname "$bin")"
    done
    chmod o+x "$scratch"
    chmod -R o+rwX "$scratch/home" "$scratch/tmp" "$scratch/profraw"
    as_unprivileged="setpriv --reuid=65534 --regid=65534 --clear-groups"
    echo "env-parity: second pass runs as uid 65534"
else
    echo "env-parity: second pass runs as $(id -un) with a perturbed environment"
fi

# no_proxy keeps loopback mock servers reachable; without it every test that
# binds 127.0.0.1 fails through the dead proxy instead of the real assertion.
run_perturbed() {
    umask 077
    # shellcheck disable=SC2086
    $as_unprivileged env -i \
        PATH=/usr/bin:/bin \
        HOME="$scratch/home" \
        TMPDIR="$scratch/tmp" \
        TZ=LNS-14 \
        LC_ALL=C \
        LANG=C \
        USER=nobody \
        LOGNAME=nobody \
        http_proxy=http://127.0.0.1:9 \
        https_proxy=http://127.0.0.1:9 \
        HTTP_PROXY=http://127.0.0.1:9 \
        HTTPS_PROXY=http://127.0.0.1:9 \
        no_proxy=127.0.0.1,localhost,::1 \
        NO_PROXY=127.0.0.1,localhost,::1 \
        LLVM_PROFILE_FILE="$scratch/profraw/%p-%m.profraw" \
        "$1"
}

ran=0
diverged=""
unrunnable=""
for bin in "$@"; do
    name=$(basename "$bin")
    if [ ! -x "$bin" ]; then
        unrunnable="$unrunnable $name"
        continue
    fi
    ran=$((ran + 1))
    if run_perturbed "$bin" >"$out" 2>&1; then
        echo "  ok   $name"
    else
        echo "  FAIL $name"
        sed 's/^/       /' "$out"
        diverged="$diverged $name"
    fi
done

# A pass that ran nothing must never read as agreement.
if [ -n "$unrunnable" ]; then
    echo "" >&2
    echo "env-parity: not executable:$unrunnable" >&2
    echo "The enumeration handed over a path the harness cannot run." >&2
    exit 1
fi

if [ "$ran" -eq 0 ]; then
    echo "env-parity: no binaries ran — refusing to report agreement" >&2
    exit 1
fi

if [ -n "$diverged" ]; then
    echo ""
    echo "env-parity: these binaries pass in one environment and fail in another:$diverged"
    echo "A test result must not depend on the host. Inject the failure through a"
    echo "seam instead of reading ambient state — see CLAUDE.md 'Environment parity'."
    exit 1
fi

echo "env-parity: $ran binaries agree across both environments"
