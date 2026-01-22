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
url="https://github.com/${REPO_OWNER}/${REPO_NAME}/releases/download/v${VERSION}/${asset}"

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

curl -fsSL -o "$workdir/$asset" "$url"

mkdir -p "$MOLT_HOME"

tar -xzf "$workdir/$asset" -C "$workdir"
extracted_dir=$(find "$workdir" -maxdepth 1 -type d -name "molt-${VERSION}*" | head -1)
if [ -z "$extracted_dir" ]; then
  echo "Failed to locate extracted bundle" >&2
  exit 1
fi

rm -rf "$MOLT_HOME"
mkdir -p "$MOLT_HOME"
cp -R "$extracted_dir"/* "$MOLT_HOME"/

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

echo "Molt installed to $MOLT_HOME"
echo "Run: molt doctor"
