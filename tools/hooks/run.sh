#!/bin/sh
# Run one molt Claude Code hook under the checkout's own interpreter.
#
# Claude Code runs a hook command through /bin/sh with the app's PATH. That
# PATH need not carry `python` (the macOS desktop app has none), and its
# `python3` may predate the hooks' imports (macOS ships 3.9). The hooks start
# Python the way the Git hooks do: .githooks/molt-hook-launch.sh selects the
# common checkout's platform-native uv environment and binds imports to this
# worktree. The interpreter runs directly: a hook runs on every Bash call, so
# no `uv run` layer and no `uv` on PATH.
#
# usage: sh tools/hooks/run.sh <hook> [argument ...]
#
# Exit 1, never 2: for a PreToolUse hook, 2 means "block this tool call", and
# a hook that cannot start must fail open with a visible error instead.

repo_root=${CLAUDE_PROJECT_DIR-}
hook=${1-}
if [ -z "$repo_root" ] || [ -z "$hook" ] || [ ! -f "$repo_root/tools/hooks/$hook.py" ]; then
    echo "molt hook ${hook:-?} did not run: no tools/hooks/${hook:-?}.py in ${repo_root:-\$CLAUDE_PROJECT_DIR}" >&2
    exit 1
fi
shift
if [ "$hook" = bash_guard ]; then
    # bash_guard.PREFILTER_TOKENS: every rule needs git, cargo or build.
    # This saves about 145 ms on each other Bash call. The test reads the raw
    # payload, while the guard reads shell words, so an escaped spelling
    # (c\argo) passes here unseen: bash_guard's gate_flip may not go strict
    # until this filter is a proven over-approximation of its rules.
    payload=$(cat)
    case $payload in
        *git* | *cargo* | *build*) ;;
        *) exit 0 ;;
    esac
fi
cd "$repo_root" || exit 1
launcher=$repo_root/.githooks/molt-hook-launch.sh
# Sourcing a missing file is fatal in POSIX sh (status 2), so check it first.
if [ ! -f "$launcher" ] || ! . "$launcher"; then
    echo "molt hook $hook did not run: $repo_root is not a Git checkout" >&2
    exit 1
fi
if [ ! -x "$vpy" ]; then
    echo "molt hook $hook did not run: provision the common checkout's uv environment ($vpy): uv sync" >&2
    exit 1
fi
if [ "$hook" = bash_guard ]; then
    printf '%s' "$payload" | "$vpy" "$repo_root/tools/hooks/$hook.py" "$@"
    exit $?
fi
exec "$vpy" "$repo_root/tools/hooks/$hook.py" "$@"
