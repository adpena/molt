#!/usr/bin/env bash
set -euo pipefail

REPO_OWNER="adpena"
REPO_NAME="molt"

MOLT_HOME_DEFAULT="$HOME/.molt"
MOLT_HOME="${MOLT_HOME:-$MOLT_HOME_DEFAULT}"
VERSION=""
UPDATE_PATH=1

usage() {
  cat <<'USAGE'
Usage: install.sh [--version X.Y.ZZZ] [--prefix PATH] [--no-path]

Environment:
  MOLT_HOME   Install root (default: ~/.molt)
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version)
      VERSION="$2"
      shift 2
      ;;
    --prefix|--home)
      MOLT_HOME="$2"
      shift 2
      ;;
    --no-path)
      UPDATE_PATH=0
      shift 1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

uname_s=$(uname -s)
uname_m=$(uname -m)

case "$uname_s" in
  Darwin)
    platform="macos"
    ;;
  Linux)
    platform="linux"
    ;;
  *)
    echo "Unsupported OS: $uname_s" >&2
    exit 1
    ;;
esac

case "$uname_m" in
  x86_64|amd64)
    arch="x86_64"
    ;;
  arm64|aarch64)
    arch="arm64"
    if [ "$platform" = "linux" ]; then
      arch="aarch64"
    fi
    ;;
  *)
    echo "Unsupported architecture: $uname_m" >&2
    exit 1
    ;;
esac

if [ -n "$VERSION" ]; then
  VERSION="${VERSION#v}"
fi

if [ -z "$VERSION" ]; then
  latest_url="https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest"
  VERSION=$(curl -fsSL "$latest_url" | grep -o '"tag_name": "v[^"]*"' | head -1 | cut -d'"' -f4 | sed 's/^v//')
  if [ -z "$VERSION" ]; then
    echo "Unable to determine latest version." >&2
    exit 1
  fi
fi

asset="molt-${VERSION}-${platform}-${arch}.tar.gz"
release_root="https://github.com/${REPO_OWNER}/${REPO_NAME}/releases/download/v${VERSION}"

workdir=$(mktemp -d)
stage="${MOLT_HOME}.new.$$"
backup="${MOLT_HOME}.old.$$"
cleanup() {
  rm -rf -- "$workdir" "$stage"
}
trap cleanup EXIT HUP INT TERM

curl --fail --silent --show-error --location \
  --retry 4 --retry-all-errors --connect-timeout 20 \
  -o "$workdir/$asset" "$release_root/$asset"
curl --fail --silent --show-error --location \
  --retry 4 --retry-all-errors --connect-timeout 20 \
  -o "$workdir/SHA256SUMS" "$release_root/SHA256SUMS"

checksum_record=$(awk -v name="$asset" '
  length($1) == 64 && $1 ~ /^[0-9a-f]+$/ && $2 == name {
    count += 1
    digest = $1
  }
  END { printf "%d:%s", count, digest }
' "$workdir/SHA256SUMS")
checksum_count=${checksum_record%%:*}
expected=${checksum_record#*:}
if [ "$checksum_count" -ne 1 ]; then
  echo "SHA256SUMS must contain exactly one digest for $asset" >&2
  exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$workdir/$asset" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
  actual=$(shasum -a 256 "$workdir/$asset" | awk '{print $1}')
else
  echo "A SHA-256 tool is required (sha256sum or shasum)." >&2
  exit 1
fi
if [ "$actual" != "$expected" ]; then
  echo "Release digest mismatch for $asset: expected $expected, got $actual" >&2
  exit 1
fi

archive_root="molt-${VERSION}"
if ! tar -tzf "$workdir/$asset" | awk -v root="$archive_root" '
  BEGIN { valid = 1; seen = 0 }
  $0 == root || index($0, root "/") == 1 { seen = 1; next }
  { valid = 0 }
  END { exit !(valid && seen) }
'; then
  echo "Release archive must contain only the $archive_root root" >&2
  exit 1
fi
tar -xzf "$workdir/$asset" -C "$workdir"
extracted_dir="$workdir/$archive_root"
if [ ! -d "$extracted_dir" ]; then
  echo "Release archive is missing the $archive_root root" >&2
  exit 1
fi

prefix_parent=$(dirname "$MOLT_HOME")
mkdir -p "$prefix_parent"
rm -rf -- "$stage" "$backup"
mkdir "$stage"
cp -R "$extracted_dir"/. "$stage"/
if [ -e "$MOLT_HOME" ]; then
  mv -- "$MOLT_HOME" "$backup"
fi
if ! mv -- "$stage" "$MOLT_HOME"; then
  if [ -e "$backup" ]; then
    mv -- "$backup" "$MOLT_HOME"
  fi
  exit 1
fi
rm -rf -- "$backup"

bin_path="$MOLT_HOME/bin"
if [ "$UPDATE_PATH" -eq 1 ]; then
  if ! echo ":$PATH:" | grep -q ":$bin_path:"; then
    shell_name=$(basename "${SHELL:-}" )
    case "$shell_name" in
      bash)
        rc="$HOME/.bashrc"
        ;;
      zsh)
        rc="$HOME/.zshrc"
        ;;
      fish)
        rc="$HOME/.config/fish/config.fish"
        ;;
      *)
        rc="$HOME/.profile"
        ;;
    esac
    mkdir -p "$(dirname "$rc")"
    if [ "$shell_name" = "fish" ]; then
      echo "set -gx PATH \"$bin_path\" \"\$PATH\"" >> "$rc"
    else
      echo "export PATH=\"$bin_path:\$PATH\"" >> "$rc"
    fi
    echo "Updated PATH in $rc"
  fi
fi

molt_bin="$bin_path/molt"
if [ ! -x "$molt_bin" ]; then
  echo "Installed bundle is missing executable: $molt_bin" >&2
  exit 1
fi

echo "Molt installed to $MOLT_HOME"
"$molt_bin" setup --strict
