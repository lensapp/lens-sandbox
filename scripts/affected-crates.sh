#!/bin/sh
# Affected-crate detection for selective coverage.
#
# USAGE: affected-crates.sh <base-ref>
#
# OUTPUT (stdout):
#   __FULL__  — run full workspace coverage
#   __NONE__  — skip coverage (docs-only or empty diff)
#   <crate>   — newline-separated bare crate names needing coverage
#
# Informational messages go to stderr with an INFO: prefix.

set -eu

info() { echo "INFO: $*" >&2; }

emit_full() {
    info "$1"
    echo "__FULL__"
    exit 0
}

base_ref=${1:-}
if [ -z "$base_ref" ]; then
    echo "usage: $0 <base-ref>" >&2
    exit 2
fi

if ! git rev-parse --verify "$base_ref" >/dev/null 2>&1; then
    emit_full "cannot resolve base ref '$base_ref' — falling back to full"
fi

if ! merge_base=$(git merge-base "$base_ref" HEAD 2>/dev/null); then
    emit_full "cannot find merge-base between '$base_ref' and HEAD — falling back to full"
fi

changed=$(git diff --name-only "$merge_base"...HEAD)

if [ -z "$changed" ]; then
    info "empty diff between $base_ref and HEAD"
    echo "__NONE__"
    exit 0
fi

full_triggered=""
while IFS= read -r path; do
    case "$path" in
        Cargo.lock)                    full_triggered=1; break ;; # dep graph change can shift which lines are compiled
        Cargo.toml)                    full_triggered=1; break ;; # workspace membership / features
        rust-toolchain.toml)           full_triggered=1; break ;; # compiler version affects codegen
        scripts/coverage-floor.sh)     full_triggered=1; break ;;
        scripts/affected-crates.sh)    full_triggered=1; break ;;
        Makefile)                      full_triggered=1; break ;;
        crates/*/Makefile)             full_triggered=1; break ;;
        crates/coverage-strip-ast/*)   full_triggered=1; break ;; # AST stripper post-processes all lcov output
        crates/e2e-tests/*)            full_triggered=1; break ;;
    esac
done <<EOF
$changed
EOF

if [ -n "$full_triggered" ]; then
    emit_full "diff contains file(s) that affect workspace-wide coverage"
fi

all_skippable=1
while IFS= read -r path; do
    case "$path" in
        *.md)                  continue ;;
        docs/*)                continue ;;
        runbooks/*)            continue ;;
        .github/*)             continue ;;
        LICENSE|LICENSE.md|LICENSE.txt) continue ;;
        .gitignore)            continue ;;
        .gitattributes)        continue ;;
        package.json)          continue ;;
        package-lock.json)     continue ;;
        .husky/*)              continue ;;
        .vscode/*)             continue ;;
        .editorconfig)         continue ;;
        *)                     all_skippable=""; break ;;
    esac
done <<EOF
$changed
EOF

if [ -n "$all_skippable" ]; then
    info "all changed paths are docs/infra — skipping coverage"
    echo "__NONE__"
    exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
    emit_full "jq not found on PATH — cannot parse cargo metadata"
fi

if ! metadata=$(cargo metadata --format-version 1 --no-deps 2>/dev/null); then
    emit_full "cargo metadata failed — cannot determine workspace members"
fi

# kind="dev" deps excluded — only normal/build deps propagate coverage scope.
workspace_members=$(echo "$metadata" | jq -r '.packages[].name')
EXCLUDED="e2e-tests coverage-strip-ast"

directly_touched=""
while IFS= read -r path; do
    case "$path" in
        crates/*/*)
            crate_dir=$(echo "$path" | cut -d/ -f2)
            if echo "$workspace_members" | grep -qx "$crate_dir"; then
                directly_touched="$directly_touched
$crate_dir"
            fi
            ;;
    esac
done <<EOF
$changed
EOF

directly_touched=$(echo "$directly_touched" | grep -v '^$' | sort -u)

if [ -z "$directly_touched" ]; then
    info "changed paths touch no workspace crates — skipping coverage"
    echo "__NONE__"
    exit 0
fi

dep_edges=$(echo "$metadata" | jq -r '
    .packages[] |
    .name as $pkg |
    .dependencies[] |
    select(.path != null) |
    select(.kind == null or .kind == "build") |
    "\($pkg) \(.name)"
')

affected="$directly_touched"
prev=""
while [ "$affected" != "$prev" ]; do
    prev="$affected"
    new_affected="$affected"
    if [ -n "$dep_edges" ]; then
        while IFS= read -r edge; do
            dependant=$(echo "$edge" | cut -d' ' -f1)
            dependency=$(echo "$edge" | cut -d' ' -f2)
            if echo "$affected" | grep -qx "$dependency"; then
                new_affected="$new_affected
$dependant"
            fi
        done <<EOF2
$dep_edges
EOF2
    fi
    affected=$(echo "$new_affected" | grep -v '^$' | sort -u)
done

result=""
while IFS= read -r crate; do
    skip=""
    for excl in $EXCLUDED; do
        if [ "$crate" = "$excl" ]; then
            skip=1
            break
        fi
    done
    if [ -z "$skip" ]; then
        result="$result
$crate"
    fi
done <<EOF
$affected
EOF

result=$(echo "$result" | grep -v '^$' | sort -u)

if [ -z "$result" ]; then
    info "all affected crates are excluded — skipping coverage"
    echo "__NONE__"
    exit 0
fi

echo "$result"
