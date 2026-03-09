#!/usr/bin/env bash

# Best-effort workspace sync for Symphony before_run hooks.
# This script must never block worker progress because a workspace is dirty.

set -u

prefix="[symphony_git_sync]"

if [[ -f tools/symphony_hooks.py ]]; then
  if command -v python3 >/dev/null 2>&1; then
    python3 tools/symphony_hooks.py before_run
    exit 0
  fi
  if command -v python >/dev/null 2>&1; then
    python tools/symphony_hooks.py before_run
    exit 0
  fi
fi

log() {
  printf "%s %s\n" "$prefix" "$*"
}

to_lower_trim() {
  # shellcheck disable=SC2001
  printf "%s" "$1" | tr '[:upper:]' '[:lower:]' | sed -E 's/^[[:space:]]+|[[:space:]]+$//g'
}

author_allowed() {
  local author="$1"
  local normalized_author
  normalized_author="$(to_lower_trim "$author")"
  if [[ -z "$normalized_author" ]]; then
    return 1
  fi

  local token normalized_token
  for token in "${ALLOWED_TOKENS[@]}"; do
    normalized_token="$(to_lower_trim "$token")"
    if [[ -z "$normalized_token" ]]; then
      continue
    fi
    if [[ "$normalized_author" == *"$normalized_token"* ]]; then
      return 0
    fi
  done
  return 1
}

REMOTE="${MOLT_SYMPHONY_SYNC_REMOTE:-origin}"
BRANCH="${MOLT_SYMPHONY_SYNC_BRANCH:-main}"
ALLOWLIST_RAW="${MOLT_SYMPHONY_AUTOMERGE_ALLOWED_AUTHORS:-adpena,symphony}"

IFS=',' read -r -a ALLOWED_TOKENS <<<"$ALLOWLIST_RAW"

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  log "skip reason=not_git_repo"
  exit 0
fi

if ! git remote get-url "$REMOTE" >/dev/null 2>&1; then
  log "skip reason=missing_remote remote=$REMOTE"
  exit 0
fi

if ! git fetch --prune "$REMOTE" "$BRANCH" >/dev/null 2>&1; then
  log "skip reason=fetch_failed remote=$REMOTE branch=$BRANCH"
  exit 0
fi

TARGET_REF="refs/remotes/$REMOTE/$BRANCH"
if ! git show-ref --verify --quiet "$TARGET_REF"; then
  log "skip reason=missing_target_ref target_ref=$TARGET_REF"
  exit 0
fi

CURRENT_BRANCH="$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
if [[ -z "$CURRENT_BRANCH" ]]; then
  log "skip reason=detached_head"
  exit 0
fi

if [[ "$CURRENT_BRANCH" != "$BRANCH" ]]; then
  if [[ -n "$(git status --porcelain --untracked-files=all)" ]]; then
    log "skip reason=dirty_workspace current_branch=$CURRENT_BRANCH target_branch=$BRANCH"
    exit 0
  fi
  if ! git checkout "$BRANCH" >/dev/null 2>&1; then
    log "skip reason=checkout_failed current_branch=$CURRENT_BRANCH target_branch=$BRANCH"
    exit 0
  fi
fi

if [[ -n "$(git status --porcelain --untracked-files=all)" ]]; then
  log "skip reason=dirty_workspace branch=$BRANCH"
  exit 0
fi

if ! git merge-base --is-ancestor HEAD "$TARGET_REF" >/dev/null 2>&1; then
  log "skip reason=not_fast_forward branch=$BRANCH target_ref=$TARGET_REF"
  exit 0
fi

if [[ "$(git rev-parse HEAD)" == "$(git rev-parse "$TARGET_REF")" ]]; then
  log "ok status=up_to_date branch=$BRANCH"
  exit 0
fi

incoming_authors=()
while IFS= read -r author; do
  if [[ -n "$author" ]]; then
    incoming_authors+=("$author")
  fi
done < <(git log --format='%an <%ae>' HEAD.."$TARGET_REF")
disallowed_authors=()
for author in "${incoming_authors[@]}"; do
  if author_allowed "$author"; then
    continue
  fi
  disallowed_authors+=("$author")
done

if [[ ${#disallowed_authors[@]} -gt 0 ]]; then
  log "skip reason=author_gate_blocked allowed=$ALLOWLIST_RAW"
  for author in "${disallowed_authors[@]}"; do
    log "blocked_author=$author"
  done
  exit 0
fi

if git merge --ff-only "$TARGET_REF" >/dev/null 2>&1; then
  log "ok status=fast_forward_applied branch=$BRANCH target_ref=$TARGET_REF"
  exit 0
fi

log "skip reason=ff_merge_failed branch=$BRANCH target_ref=$TARGET_REF"
exit 0
