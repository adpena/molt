#!/usr/bin/env bash

# Best-effort auto-commit/push hook for Symphony after_run.
# This hook should accelerate landing work but never block worker progress.

set -u

prefix="[symphony_autoland]"

if [[ -f tools/symphony_hooks.py ]]; then
  if command -v python3 >/dev/null 2>&1; then
    python3 tools/symphony_hooks.py after_run
    exit 0
  fi
  if command -v python >/dev/null 2>&1; then
    python tools/symphony_hooks.py after_run
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

env_enabled() {
  local raw normalized
  raw="${1:-}"
  normalized="$(to_lower_trim "$raw")"
  case "$normalized" in
    1|true|yes|on) return 0 ;;
    *) return 1 ;;
  esac
}

token_match() {
  local haystack="$1"
  local normalized_haystack normalized_token token
  normalized_haystack="$(to_lower_trim "$haystack")"
  if [[ -z "$normalized_haystack" ]]; then
    return 1
  fi
  for token in "${TOKENS[@]}"; do
    normalized_token="$(to_lower_trim "$token")"
    if [[ -z "$normalized_token" ]]; then
      continue
    fi
    if [[ "$normalized_haystack" == *"$normalized_token"* ]]; then
      return 0
    fi
  done
  return 1
}

machine_allowed() {
  local machine="$1"
  local normalized_machine normalized_token token
  normalized_machine="$(to_lower_trim "$machine")"
  if [[ -z "$normalized_machine" ]]; then
    return 1
  fi
  for token in "${TRUSTED_MACHINES[@]}"; do
    normalized_token="$(to_lower_trim "$token")"
    if [[ -z "$normalized_token" ]]; then
      continue
    fi
    if [[ "$normalized_machine" == *"$normalized_token"* ]]; then
      return 0
    fi
  done
  return 1
}

AUTOLAND_ENABLED="${MOLT_SYMPHONY_AUTOLAND_ENABLED:-1}"
AUTOLAND_MODE="${MOLT_SYMPHONY_AUTOLAND_MODE:-direct-main}"
REMOTE="${MOLT_SYMPHONY_SYNC_REMOTE:-origin}"
TARGET_BRANCH="${MOLT_SYMPHONY_SYNC_BRANCH:-main}"
COMMIT_MESSAGE="${MOLT_SYMPHONY_AUTOLAND_COMMIT_MESSAGE:-chore: sync all changes}"
ALLOWLIST_RAW="${MOLT_SYMPHONY_AUTOMERGE_ALLOWED_AUTHORS:-adpena,symphony}"
TRUSTED_USERS_RAW="${MOLT_SYMPHONY_TRUSTED_USERS:-adpena,symphony}"
TRUSTED_MACHINES_RAW="${MOLT_SYMPHONY_TRUSTED_MACHINES:-}"
PR_BASE="${MOLT_SYMPHONY_AUTOLAND_PR_BASE:-main}"
PR_AUTOMERGE="${MOLT_SYMPHONY_AUTOLAND_PR_AUTOMERGE:-1}"

if ! env_enabled "$AUTOLAND_ENABLED"; then
  log "skip reason=disabled"
  exit 0
fi

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  log "skip reason=not_git_repo"
  exit 0
fi

current_branch="$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
if [[ -z "$current_branch" ]]; then
  log "skip reason=detached_head"
  exit 0
fi

if [[ -z "$(git status --porcelain --untracked-files=all)" ]]; then
  log "skip reason=no_changes branch=$current_branch"
  exit 0
fi

IFS=',' read -r -a ALLOWLIST_TOKENS <<<"$ALLOWLIST_RAW"
IFS=',' read -r -a TRUSTED_USER_TOKENS <<<"$TRUSTED_USERS_RAW"
IFS=',' read -r -a TRUSTED_MACHINES <<<"$TRUSTED_MACHINES_RAW"

author_name="$(git config --get user.name || true)"
author_email="$(git config --get user.email || true)"
author_identity="$author_name <$author_email>"
machine_identity="$(hostname 2>/dev/null || uname -n 2>/dev/null || true)"

TOKENS=("${ALLOWLIST_TOKENS[@]}")
author_allowed=0
if token_match "$author_identity"; then
  author_allowed=1
else
  TOKENS=("${TRUSTED_USER_TOKENS[@]}")
  if token_match "$author_identity"; then
    author_allowed=1
  fi
fi

machine_ok=0
if machine_allowed "$machine_identity"; then
  machine_ok=1
fi

if [[ "$author_allowed" -ne 1 && "$machine_ok" -ne 1 ]]; then
  log "skip reason=untrusted_identity author=$author_identity machine=$machine_identity"
  exit 0
fi

if [[ "$AUTOLAND_MODE" != "direct-main" && "$AUTOLAND_MODE" != "pr-automerge" ]]; then
  log "skip reason=unsupported_mode mode=$AUTOLAND_MODE"
  exit 0
fi

if [[ "$AUTOLAND_MODE" == "direct-main" && "$current_branch" != "$TARGET_BRANCH" ]]; then
  log "skip reason=branch_mismatch mode=direct-main current=$current_branch target=$TARGET_BRANCH"
  exit 0
fi

git add -A

if [[ -z "$(git diff --cached --name-only)" ]]; then
  log "skip reason=nothing_staged"
  exit 0
fi

if command -v python3 >/dev/null 2>&1 && [[ -f tools/secret_guard.py ]]; then
  if ! PYTHONPATH="${PYTHONPATH:-src}" python3 tools/secret_guard.py --staged >/dev/null 2>&1; then
    log "skip reason=secret_guard_blocked"
    exit 0
  fi
fi

if ! git commit -m "$COMMIT_MESSAGE" >/dev/null 2>&1; then
  log "skip reason=commit_failed"
  exit 0
fi

if ! git push "$REMOTE" "$current_branch" >/dev/null 2>&1; then
  if git pull --rebase "$REMOTE" "$current_branch" >/dev/null 2>&1; then
    if ! git push "$REMOTE" "$current_branch" >/dev/null 2>&1; then
      log "skip reason=push_failed_after_rebase branch=$current_branch"
      exit 0
    fi
  else
    log "skip reason=rebase_failed branch=$current_branch"
    exit 0
  fi
fi

if [[ "$AUTOLAND_MODE" != "pr-automerge" ]]; then
  log "ok status=pushed mode=direct-main branch=$current_branch"
  exit 0
fi

if ! command -v gh >/dev/null 2>&1; then
  log "ok status=pushed mode=pr-automerge skip_reason=gh_missing branch=$current_branch"
  exit 0
fi

if [[ "$current_branch" == "$PR_BASE" ]]; then
  log "ok status=pushed mode=pr-automerge skip_reason=already_on_base branch=$current_branch"
  exit 0
fi

pr_title="${MOLT_SYMPHONY_AUTOLAND_PR_TITLE:-$COMMIT_MESSAGE}"
pr_body="${MOLT_SYMPHONY_AUTOLAND_PR_BODY:-Automated Symphony autoland from trusted identity.}"

if ! gh pr create --base "$PR_BASE" --head "$current_branch" --title "$pr_title" --body "$pr_body" >/dev/null 2>&1; then
  log "ok status=pushed mode=pr-automerge skip_reason=pr_create_failed branch=$current_branch"
  exit 0
fi

if env_enabled "$PR_AUTOMERGE"; then
  if gh pr merge "$current_branch" --auto --squash --delete-branch >/dev/null 2>&1; then
    log "ok status=pr_automerge_queued branch=$current_branch base=$PR_BASE"
    exit 0
  fi
  log "ok status=pr_created mode=pr-automerge skip_reason=merge_queue_failed branch=$current_branch base=$PR_BASE"
  exit 0
fi

log "ok status=pr_created mode=pr-automerge branch=$current_branch base=$PR_BASE"
exit 0
