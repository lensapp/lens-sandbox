#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
upstream="$root/target/openshell"
pin=02b664bb0d978ac0baec9aaa0bf06a2a4f67e83d
patch="$root/spikes/openshell-network/openshell.patch"
if [ ! -d "$upstream/.git" ]; then
    git clone --no-checkout "${OPENSHELL_SOURCE:-https://github.com/NVIDIA/OpenShell}" "$upstream"
    git -C "$upstream" checkout --detach "$pin"
fi
test "$(git -C "$upstream" rev-parse HEAD)" = "$pin" || { echo 'Unexpected OpenShell revision' >&2; exit 1; }
if git -C "$upstream" apply --reverse --check "$patch" 2>/dev/null; then
    exit 0
fi
git -C "$upstream" diff --quiet || { echo 'OpenShell checkout has unexpected edits' >&2; exit 1; }
git -C "$upstream" apply --check "$patch"
git -C "$upstream" apply "$patch"
