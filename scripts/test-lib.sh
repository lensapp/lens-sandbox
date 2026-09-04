# Sourced by the shell harnesses. Not itself a harness: `scripts/*.test.sh`
# does not match this name.

# A fixture must not read the host. GIT_CONFIG_GLOBAL needs git 2.32, so HOME
# and GIT_CONFIG_NOSYSTEM cover the older versions this repo still fits.
GIT_HOME=$(mktemp -d)
mkdir -p "$GIT_HOME/.config"
: >"$GIT_HOME/.gitconfig"

# The toolchain homes default to $HOME, so pin them to the real one before it
# moves — a fixture that shells out to cargo must not find a cold toolchain.
# Guarded: a harness must not start requiring a home it never needed.
if [ -n "${HOME:-}" ]; then
    export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
    export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
fi

# A fixture owns the repositories it makes and inherits no git environment: a
# linked worktree hands its hooks an absolute GIT_DIR, so under pre-push an
# inherited one puts every fixture commit on the branch being pushed.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_PREFIX \
    GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE \
    GIT_QUARANTINE_PATH GIT_PUSH_CERT_NONCE GIT_REFLOG_ACTION \
    GIT_CONFIG GIT_CONFIG_SYSTEM GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT \
    GIT_TEMPLATE_DIR GIT_AUTHOR_DATE GIT_COMMITTER_DATE

export HOME="$GIT_HOME"
export XDG_CONFIG_HOME="$GIT_HOME/.config"
export GIT_CONFIG_GLOBAL="$GIT_HOME/.gitconfig"
export GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=Test
export GIT_AUTHOR_EMAIL=test@test.local
export GIT_COMMITTER_NAME=Test
export GIT_COMMITTER_EMAIL=test@test.local

# Frees what this file allocated; every harness's cleanup calls it.
test_lib_cleanup() {
    rm -rf "$GIT_HOME"
}
