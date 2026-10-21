#!/bin/sh
# Run one molt Claude Code hook under the checkout's own interpreter.
#
# Claude Code runs a hook command through /bin/sh with the app's PATH. That
# PATH need not carry `python` (the macOS desktop app has none), and its
# `python3` may predate the hooks' imports (macOS ships 3.9). The hooks start
# Python the way the Git hooks do: .githooks/molt-hook-launch.sh selects the
# common checkout's platform-native uv environment and binds imports to this
# worktree. The interpreter runs directly: no `uv run` layer, no `uv` on
# PATH, and no environment sync at session start.
#
# usage: sh tools/hooks/run.sh <hook> [argument ...]
#
# Exit 1, never 2: a hook that cannot start must fail open with a visible
# error instead of blocking anything.

repo_root=${CLAUDE_PROJECT_DIR-}
hook=${1-}
if [ -z "$repo_root" ] || [ -z "$hook" ] || [ ! -f "$repo_root/tools/hooks/$hook.py" ]; then
    echo "molt hook ${hook:-?} did not run: no tools/hooks/${hook:-?}.py in ${repo_root:-\$CLAUDE_PROJECT_DIR}" >&2
    exit 1
fi
shift
cd "$repo_root" || exit 1
launcher=$repo_root/.githooks/molt-hook-launch.sh
# Sourcing a missing file is fatal in POSIX sh (status 2), so check it first.
if [ ! -f "$launcher" ]; then
    echo "molt hook $hook did not run: no .githooks/molt-hook-launch.sh in $repo_root" >&2
    exit 1
fi
if ! . "$launcher"; then
    echo "molt hook $hook did not run: $repo_root is not a Git checkout" >&2
    exit 1
fi
if [ ! -x "$vpy" ]; then
    echo "molt hook $hook did not run: provision the common checkout's uv environment ($vpy): uv sync" >&2
    exit 1
fi
exec "$vpy" "$repo_root/tools/hooks/$hook.py" "$@"
