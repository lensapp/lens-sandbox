# Shared by the hooks in this directory. Not a hook itself: git runs only the
# files it has names for.

# node_modules is untracked, so a fresh worktree has none. Fall back to the
# main worktree's, which every worktree of the clone can reach. Never fails —
# an absent tool is a skip, and `set -e` would turn that into a refusal.
node_bin() {
    tool=$1
    if [ -x "node_modules/.bin/$tool" ]; then
        echo "node_modules/.bin/$tool"
        return 0
    fi
    main=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null | tail -n 1)")
    case "$main" in
        /*) ;;
        *) return 0 ;;
    esac
    # A bare clone or --separate-git-dir puts the git dir beside an unrelated
    # directory; only a real work-tree root may lend its tools.
    [ "$(git -C "$main" rev-parse --show-toplevel 2>/dev/null)" = "$main" ] || return 0
    if [ -x "$main/node_modules/.bin/$tool" ]; then
        echo "$main/node_modules/.bin/$tool"
    fi
    return 0
}
