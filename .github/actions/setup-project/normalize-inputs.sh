#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: normalize-inputs.sh GITHUB_OUTPUT" >&2
  exit 2
fi

output=$1

validate_bool() {
  local label=$1 value=$2
  if [[ "$value" != "true" && "$value" != "false" ]]; then
    echo "$label must be exactly true or false" >&2
    exit 2
  fi
}

validate_atom() {
  local label=$1 value=$2
  if [[ -z "$value" || ! "$value" =~ ^[A-Za-z0-9._+-]+$ ]]; then
    echo "invalid $label atom: $value" >&2
    exit 2
  fi
}

normalize_list() {
  local label=$1 raw=$2 token canonical=""
  local -a values=() normalized=()
  if [[ "$raw" == *$'\n'* || "$raw" == *$'\r'* || "$raw" == *$'\t'* ]]; then
    echo "$label contains a control character" >&2
    exit 2
  fi
  if [[ -n "$raw" ]]; then
    IFS=',' read -r -a values <<< "$raw"
    for token in "${values[@]}"; do
      token="${token#"${token%%[![:space:]]*}"}"
      token="${token%"${token##*[![:space:]]}"}"
      validate_atom "$label" "$token"
      normalized+=("$token")
    done
    while IFS= read -r token; do
      [[ -z "$canonical" ]] || canonical+=,
      canonical+="$token"
    done < <(printf '%s\n' "${normalized[@]}" | LC_ALL=C sort -u)
  fi
  printf '%s' "$canonical"
}

python=${INPUT_PYTHON:?}
uv=${INPUT_UV:?}
cache_uv=${INPUT_CACHE_UV:?}
cache_cargo=${INPUT_CACHE_CARGO:?}
cache_lean=${INPUT_CACHE_LEAN:?}
actionlint=${INPUT_ACTIONLINT:?}
sync=${INPUT_SYNC:?}
sync_frozen=${INPUT_SYNC_FROZEN:?}
sync_dev=${INPUT_SYNC_DEV:?}
namespace=${INPUT_CACHE_NAMESPACE:?}

# Callers name a role, never a version: each version lives in one authority
# file, so a pin bump edits that file and no workflow.
read_exact_version() {
  local label=$1 version=$2
  if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "$label must declare exactly one X.Y.Z version, got: $version" >&2
    exit 2
  fi
  printf '%s' "$version"
}

toolchain=${INPUT_RUST_TOOLCHAIN:-}
case "$toolchain" in
  "") ;;
  pinned)
    toolchain=$(read_exact_version "rust-toolchain.toml channel" \
      "$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)")
    ;;
  sanitizer-nightly)
    toolchain=$(< config/rust_nightly_toolchain.txt)
    ;;
  *)
    echo "rust-toolchain must be 'pinned' (rust-toolchain.toml) or 'sanitizer-nightly'" \
      "(config/rust_nightly_toolchain.txt), got: $toolchain" >&2
    exit 2
    ;;
esac

node_version=${INPUT_NODE_VERSION:-}
case "$node_version" in
  "") ;;
  pinned)
    node_version=$(read_exact_version "config/tool_releases.toml [tools.node] version" \
      "$(sed -n '/^\[tools\.node\]$/,/^\[/s/^version = "\(.*\)"$/\1/p' config/tool_releases.toml)")
    ;;
  *)
    echo "node-version must be 'pinned' (config/tool_releases.toml), got: $node_version" >&2
    exit 2
    ;;
esac
if [[ -n "${INPUT_NODE_CACHE_DEPENDENCY_PATH:-}" && -z "$node_version" ]]; then
  echo "node-cache-dependency-path requires node-version" >&2
  exit 2
fi

for bool_name in python uv cache_uv cache_cargo cache_lean actionlint sync sync_frozen sync_dev; do
  validate_bool "${bool_name//_/-}" "${!bool_name}"
done
validate_atom cache-namespace "$namespace"

components=$(normalize_list rust-component "${INPUT_RUST_COMPONENTS:-}")
targets=$(normalize_list rust-target "${INPUT_RUST_TARGETS:-}")
sync_groups=$(normalize_list sync-group "${INPUT_SYNC_GROUPS:-}")

if [[ "$cache_uv" == "true" && "$uv" != "true" ]]; then
  echo "cache-uv requires uv" >&2
  exit 2
fi
if [[ "$sync" == "true" && "$uv" != "true" ]]; then
  echo "sync requires uv" >&2
  exit 2
fi
if [[ "$sync" != "true" && ( "$sync_frozen" == "true" || "$sync_dev" == "true" || -n "$sync_groups" ) ]]; then
  echo "sync options require sync" >&2
  exit 2
fi
if [[ "$cache_cargo" == "true" && -z "$toolchain" ]]; then
  echo "cache-cargo requires rust-toolchain" >&2
  exit 2
fi
if [[ "$actionlint" == "true" && "$python" != "true" ]]; then
  echo "actionlint requires python" >&2
  exit 2
fi
if [[ -z "$toolchain" && ( -n "$components" || -n "$targets" ) ]]; then
  echo "Rust components and targets require rust-toolchain" >&2
  exit 2
fi
if [[ -n "$toolchain" ]]; then
  validate_atom rust-toolchain "$toolchain"
fi

# Git is guaranteed on every supported Actions runner. Hash a length-delimited
# canonical tuple so cache keys never embed caller punctuation or list order.
# The job id is part of the identity: jobs build different crate sets into
# different target directories, and actions/cache folds paths into the cache
# version, so a key shared across jobs can never restore for most of them.
rust_cache_token=$(
  printf '%s\0%s\0%s\0%s\0%s\0' "$toolchain" "$components" "$targets" "$namespace" \
    "${GITHUB_JOB:?}" | git hash-object --stdin
)

{
  printf 'python=%s\n' "$python"
  printf 'uv=%s\n' "$uv"
  printf 'cache-uv=%s\n' "$cache_uv"
  printf 'cache-cargo=%s\n' "$cache_cargo"
  printf 'cache-lean=%s\n' "$cache_lean"
  printf 'actionlint=%s\n' "$actionlint"
  printf 'sync=%s\n' "$sync"
  printf 'sync-frozen=%s\n' "$sync_frozen"
  printf 'sync-dev=%s\n' "$sync_dev"
  printf 'sync-groups=%s\n' "$sync_groups"
  printf 'cache-namespace=%s\n' "$namespace"
  printf 'rust-toolchain=%s\n' "$toolchain"
  printf 'node-version=%s\n' "$node_version"
  printf 'rust-components=%s\n' "$components"
  printf 'rust-targets=%s\n' "$targets"
  printf 'rust-cache-token=%s\n' "$rust_cache_token"
} >> "$output"
