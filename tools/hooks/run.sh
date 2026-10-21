#!/bin/sh
# Run one molt Claude Code hook under the checkout's own interpreter.
#
# Claude Code runs a hook command through /bin/sh with the app's PATH. That
# PATH need not carry `python` (the macOS desktop app has none), and its
# `python3` may predate the hooks' imports (macOS ships 3.9; the hooks import
# repository tools that need the project's 3.12 floor). So every hook runs
# under the interpreter of the checkout's uv environment, the one each
# repository tool runs under, found by uv's own rule: UV_PROJECT_ENVIRONMENT,
# relative to the checkout unless absolute, else `.venv`.
#
# usage: sh tools/hooks/run.sh <hook> [argument ...]
#
# Exit 1, never 2: for a PreToolUse hook, 2 means "block this tool call", and
# a hook that cannot start must fail open with a visible error instead.

root=${CLAUDE_PROJECT_DIR-}
if [ -z "$root" ] || [ "$#" -eq 0 ]; then
    echo "usage: CLAUDE_PROJECT_DIR=<checkout> sh tools/hooks/run.sh <hook> [argument ...]" >&2
    exit 1
fi
hook=$1
shift
environment=${UV_PROJECT_ENVIRONMENT:-.venv}
case $environment in
    /* | [A-Za-z]:[\\/]*) ;;
    *) environment=$root/$environment ;;
esac
python=$environment/bin/python
if [ ! -x "$python" ]; then
    # A Windows virtual environment keeps its interpreter under Scripts.
    python=$environment/Scripts/python.exe
fi
if [ ! -x "$python" ]; then
    echo "molt hook $hook did not run: no interpreter in $environment; run 'uv sync' in $root" >&2
    exit 1
fi
exec "$python" "$root/tools/hooks/$hook.py" "$@"
