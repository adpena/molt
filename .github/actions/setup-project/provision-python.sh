#!/usr/bin/env bash
# This bootstrap runs before any repository Python module is imported.
set -euo pipefail

single_line() {
  local label=$1 value=$2
  if [[ -z "$value" || "$value" =~ [[:cntrl:]] ]]; then
    echo "$label must be a nonempty single line" >&2
    exit 2
  fi
}

root=${MOLT_CI_EPHEMERAL_CUSTODY_ROOT:?}
single_line custody-root "$root"
single_line GITHUB_ENV "${GITHUB_ENV:?}"
single_line GITHUB_PATH "${GITHUB_PATH:?}"
single_line GITHUB_OUTPUT "${GITHUB_OUTPUT:?}"
case "$root" in
  /* | [A-Za-z]:[\\/]*) ;;
  *) echo "custody-root must be absolute" >&2; exit 2 ;;
esac
case "${RUNNER_OS:?}" in
  Windows | Linux | macOS) ;;
  *) echo "unsupported Actions runner OS" >&2; exit 2 ;;
esac

# The sentinel preserves trailing newlines so two pins or blank lines cannot
# accidentally become a valid version through shell command substitution.
pin=$(cat .python-version; printf '.')
pin=${pin%.}
pin=${pin%$'\n'}
pin=${pin%$'\r'}
if [[ ! "$pin" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo ".python-version must contain exactly one X.Y.Z version" >&2
  exit 2
fi
export UV_PYTHON_INSTALL_DIR="$root/python"
export UV_PYTHON_BIN_DIR="$root/python-bin"
export UV_MANAGED_PYTHON=true
# Downloads belong to this explicit setup phase, never later test discovery.
unset UV_PYTHON_DOWNLOADS
uv python install --default "$pin"
selected=$(uv python find --managed-python --no-project --no-python-downloads "$pin")
bin=$(uv python dir --bin)
single_line selected-python "$selected"
single_line python-bin "$bin"

selected_command=$selected
bin_command=$bin
case "${RUNNER_OS:?}" in
  Windows)
    selected_command=$(cygpath -u "$selected")
    bin_command=$(cygpath -u "$bin")
    ;;
  Linux | macOS) ;;
esac

# Validate the selected executable and both user-facing aliases. Do not assume
# an installation layout or manufacture python3.exe on Windows.
check='import os, pathlib, platform, sys
pin, selected, install = sys.argv[1:]
if platform.python_implementation() != "CPython" or platform.python_version() != pin:
    raise SystemExit("managed Python implementation/version mismatch")
actual = pathlib.Path(sys.executable).resolve(strict=True)
actual.relative_to(pathlib.Path(install).resolve(strict=True))
if not os.path.samefile(actual, selected):
    raise SystemExit("Python alias does not select the admitted interpreter")
print(str(actual))'
"$selected_command" -I -S -B -c "$check" "$pin" "$selected" "$UV_PYTHON_INSTALL_DIR"
PATH="$bin_command:$PATH" python -I -S -B -c "$check" "$pin" "$selected" "$UV_PYTHON_INSTALL_DIR"
PATH="$bin_command:$PATH" python3 -I -S -B -c "$check" "$pin" "$selected" "$UV_PYTHON_INSTALL_DIR"

# Nothing is published until every validation succeeds. Values used in these
# single-line Actions records have already rejected control characters.
{
  printf 'UV_PYTHON_INSTALL_DIR=%s\n' "$UV_PYTHON_INSTALL_DIR"
  printf 'UV_PYTHON_BIN_DIR=%s\n' "$UV_PYTHON_BIN_DIR"
  printf 'UV_PYTHON=%s\n' "$selected"
  printf 'UV_MANAGED_PYTHON=true\nUV_PYTHON_DOWNLOADS=never\n'
} >> "$GITHUB_ENV"
printf '%s\n' "$bin" >> "$GITHUB_PATH"
printf 'python-path=%s\npython-version=%s\n' "$selected" "$pin" >> "$GITHUB_OUTPUT"
