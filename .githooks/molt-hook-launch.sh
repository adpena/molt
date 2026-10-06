# molt-hook-launch v1 — sourced by Molt's Git hooks; do not run it directly.
#
# One authority for how a hook starts Python: bind startup imports to the
# invoking worktree (an installed interpreter may be shared across worktrees),
# select the common checkout's platform-native uv environment, and run a
# repository tool under uv with no project sync, dependency resolution, or
# editable installation. Callers set `repo_root` before sourcing this file.

common_dir="$(git rev-parse --git-common-dir 2>/dev/null)" || return 1
main_root="$(cd "$(dirname "$common_dir")" 2>/dev/null && pwd)" || return 1
case "$OSTYPE" in
  msys*|cygwin*)
    python_root="$(cygpath -m "$repo_root")" || return 1
    export PYTHONPATH="$python_root/src;$python_root"
    vpy="$main_root/.venv/Scripts/python.exe"
    ;;
  *)
    export PYTHONPATH="$repo_root/src:$repo_root"
    vpy="$main_root/.venv/bin/python"
    ;;
esac
export PYTHONNOUSERSITE=1
unset PYTHONHOME

# molt_hook_uv_python <hook-name> <tool.py> [args...]
molt_hook_uv_python() {
  hook_name="$1"
  shift
  if ! command -v uv >/dev/null 2>&1; then
    echo "$hook_name: uv is required to run $1" >&2
    return 1
  fi
  if [ ! -x "$vpy" ]; then
    echo "$hook_name: provision the common checkout's platform-native uv environment: $vpy" >&2
    return 1
  fi
  uv run --no-project --offline --no-config --python "$vpy" python "$@"
}
