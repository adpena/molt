#!/usr/bin/env bash
# Shared shell/host boundary helpers for Molt wrappers.

molt_shell_is_wsl_windows_mount() {
  case "$1" in
    /mnt/[A-Za-z]/*) return 0 ;;
    *) return 1 ;;
  esac
}

molt_select_python() {
  local root
  root="$1"
  if [ -x "$root/.venv/Scripts/python.exe" ]; then
    printf '%s\n' "$root/.venv/Scripts/python.exe"
    return
  fi
  if molt_shell_is_wsl_windows_mount "$root" && command -v python.exe >/dev/null 2>&1; then
    printf '%s\n' python.exe
    return
  fi
  if command -v cygpath >/dev/null 2>&1 && command -v python.exe >/dev/null 2>&1; then
    command -v python.exe
    return
  fi
  if [ -x "$root/.venv/bin/python" ]; then
    printf '%s\n' "$root/.venv/bin/python"
    return
  fi
  # uv selects for the project's requires-python, as every documented command
  # does. A PATH interpreter can be older (macOS ships python3 3.9).
  local found candidate
  if command -v uv >/dev/null 2>&1 && found="$(cd "$root" && uv python find 2>/dev/null)" && [ -n "$found" ]; then
    printf '%s\n' "$found"
    return
  fi
  for candidate in python3 python; do
    if command -v "$candidate" >/dev/null 2>&1 \
      && "$candidate" -B -c 'import sys; raise SystemExit(sys.version_info < (3, 12))' >/dev/null 2>&1; then
      command -v "$candidate"
      return
    fi
  done
  echo "Molt shell wrappers require CPython 3.12 or newer; install uv or put one on PATH." >&2
  return 127
}

molt_python_uses_windows_paths() {
  case "$1" in
    *.exe | *python.exe) return 0 ;;
    *) return 1 ;;
  esac
}

molt_command_uses_windows_paths() {
  case "$1" in
    *.exe | *.EXE) return 0 ;;
    *) return 1 ;;
  esac
}

molt_host_path() {
  local path
  path="$1"
  case "$path" in
    [A-Za-z]:/* | [A-Za-z]:\\* | \\\\*) printf '%s\n' "$path"; return ;;
  esac
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -m "$path"
    return
  fi
  if command -v wslpath >/dev/null 2>&1; then
    wslpath -m "$path"
    return
  fi
  printf '%s\n' "$path"
}

molt_host_path_for_python() {
  local path python
  path="$1"
  python="$2"
  if molt_python_uses_windows_paths "$python"; then
    molt_host_path "$path"
    return
  fi
  printf '%s\n' "$path"
}

molt_shell_path() {
  local path
  path="$1"
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -u "$path" 2>/dev/null || printf '%s\n' "$path"
    return
  fi
  if command -v wslpath >/dev/null 2>&1; then
    wslpath -u "$path" 2>/dev/null || printf '%s\n' "$path"
    return
  fi
  printf '%s\n' "$path"
}

molt_path_for_command() {
  local path command_path
  path="$1"
  command_path="$2"
  if molt_command_uses_windows_paths "$command_path"; then
    molt_host_path "$path"
    return
  fi
  molt_shell_path "$path"
}

molt_init_shell_context() {
  local root
  root="$1"
  MOLT_SHELL_ROOT="$root"
  MOLT_PYTHON="$(molt_select_python "$MOLT_SHELL_ROOT")"
  MOLT_HOST_ROOT="$(molt_host_path_for_python "$MOLT_SHELL_ROOT" "$MOLT_PYTHON")"
  export MOLT_SHELL_ROOT MOLT_PYTHON MOLT_HOST_ROOT
}

# Print the host path of one purpose's run scratch (molt.dx.scratch_dir):
# never inside the checkout, and on the selected scratch storage.
molt_scratch_dir() {
  local root purpose python host_root
  root="$1"
  purpose="$2"
  python="${MOLT_PYTHON:-$(molt_select_python "$root")}"
  host_root="$(molt_host_path_for_python "$root" "$python")"
  PYTHONPATH="$host_root/src${PYTHONPATH:+:$PYTHONPATH}" \
    "$python" -B -c 'import sys; from pathlib import Path; from molt.dx import scratch_dir; print(scratch_dir(Path(sys.argv[1]), sys.argv[2]))' \
    "$host_root" "$purpose"
}

molt_run_context_env() {
  local root python host_root
  root="$1"
  shift
  python="${MOLT_PYTHON:-$(molt_select_python "$root")}"
  host_root="$(molt_host_path_for_python "$root" "$python")"
  if [ -n "${MOLT_SESSION_ID:-}" ]; then
    set -- --session-id "$MOLT_SESSION_ID" "$@"
  fi
  PYTHONPATH="$host_root/src${PYTHONPATH:+:$PYTHONPATH}" \
    "$python" -B "$host_root/tools/run_context_env.py" --root "$host_root" "$@"
}
