#!/usr/bin/env bash
# Sourced by run_stack.sh; owns only this shell's service groups.
# Each background service gets its own process group. Only groups created by
# this shell are eligible for cleanup, including server-spawned worker clients.
set -m
SERVICE_PIDS=()
cleanup() {
  local status=$? pid
  trap - EXIT INT TERM
  # Stop the server first so it can close its worker clients gracefully.
  for ((i=${#SERVICE_PIDS[@]}-1; i>=0; i--)); do
    pid="${SERVICE_PIDS[i]}"
    kill -TERM "$pid" 2>/dev/null || true
    for _ in {1..100}; do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.1
    done
    # A wrapper exit is not proof that its descendants exited. Drain the
    # service's private group before reaping our direct child.
    kill -TERM -- "-$pid" 2>/dev/null || true
    for _ in {1..20}; do
      kill -0 -- "-$pid" 2>/dev/null || break
      sleep 0.1
    done
    kill -KILL -- "-$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  if (( status != 0 )); then
    for log in "$ROOT/logs/molt_django.log" "$ROOT/logs/molt_worker.log"; do
      [[ ! -f "$log" ]] || tail -n 40 "$log" >&2
    done
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
