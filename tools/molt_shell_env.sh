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
  if command -v python3 >/dev/null 2>&1; then
    command -v python3
    return
  fi
  if command -v python >/dev/null 2>&1; then
    command -v python
    return
  fi
  echo "Molt shell wrappers require Python; none was found on PATH or in .venv." >&2
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
    "$python" "$host_root/tools/run_context_env.py" --root "$host_root" "$@"
}
