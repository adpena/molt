from __future__ import annotations

import os
import queue
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

try:  # pragma: no cover - platform dependent
    import resource
except ImportError:  # pragma: no cover - Windows/no-resource
    resource = None  # type: ignore[assignment]

from .app_server import CodexAppServerClient
from .config import (
    build_runtime_config,
    normalize_state_list,
    normalize_state_name,
    validate_dispatch_config,
)
from .errors import (
    AgentRunnerError,
    ConfigValidationError,
    HookError,
    MissingWorkflowFileError,
    SymphonyError,
    TrackerError,
    TurnCancelledError,
    TurnFailedError,
    TurnInputRequiredError,
    TurnTimeoutError,
    WorkflowFrontMatterNotMapError,
    WorkflowParseError,
)
from .http_server import DashboardServer
from .linear import LinearTrackerClient, blocker_allows_todo
from .logging_utils import log
from .models import (
    Issue,
    OrchestratorState,
    RetryEntry,
    RunningEntry,
    RuntimeConfig,
    WorkflowDefinition,
    now_utc,
)
from .profiling import profiled
from .template import render_prompt
from .workflow import load_workflow, maybe_reload_workflow
from .workspace import WorkspaceManager, sanitize_workspace_key


@dataclass(slots=True)
class WorkerControl:
    thread: threading.Thread
    stop_event: threading.Event


class SymphonyOrchestrator:
    def __init__(
        self,
        workflow: WorkflowDefinition,
        config: RuntimeConfig,
        *,
        port_override: int | None = None,
        run_once: bool = False,
    ) -> None:
        self._workflow = workflow
        self._config = config
        self._tracker = LinearTrackerClient(config.tracker)
        self._workspace = WorkspaceManager(config.workspace, config.hooks)
        self._port_override = port_override
        self._run_once = run_once
        self._exec_mode = (
            str(os.environ.get("MOLT_SYMPHONY_EXEC_MODE", "python")).strip() or "python"
        )

        self._state = OrchestratorState(
            poll_interval_ms=config.polling.interval_ms,
            max_concurrent_agents=config.agent.max_concurrent_agents,
        )
        self._state_lock = threading.Lock()

        self._worker_controls: dict[str, WorkerControl] = {}
        self._event_queue: queue.Queue[tuple[str, dict[str, Any]]] = queue.Queue()

        self._active_states = normalize_state_list(self._config.tracker.active_states)
        self._terminal_states = normalize_state_list(
            self._config.tracker.terminal_states
        )

        self._stop_event = threading.Event()
        self._refresh_event = threading.Event()
        self._wake_event = threading.Event()

        self._server: DashboardServer | None = None
        self._auth_resume_delay_seconds = max(
            int(os.environ.get("MOLT_SYMPHONY_AUTH_RESUME_DELAY_SECONDS", "300")),
            30,
        )
        self._rate_limit_resume_default_seconds = max(
            int(os.environ.get("MOLT_SYMPHONY_RATE_LIMIT_RESUME_SECONDS", "900")),
            60,
        )

    def run(self) -> int:
        self._startup_terminal_workspace_cleanup()
        self._start_http_server_if_enabled()

        next_tick = 0.0
        tick_count = 0
        log("INFO", "symphony_started", workflow_path=self._workflow.path)

        try:
            while not self._stop_event.is_set():
                with self._state_lock:
                    self._state.profiling.incr("event_loop_iterations")
                    self._state.profiling.observe_queue_depth(self._event_queue.qsize())
                self._drain_events()
                self._process_due_retries()
                now = time.monotonic()

                if self._refresh_event.is_set():
                    self._refresh_event.clear()
                    next_tick = 0.0

                if now >= next_tick:
                    tick_count += 1
                    self._tick()
                    if self._run_once:
                        break
                    with self._state_lock:
                        poll_interval = self._state.poll_interval_ms
                    next_tick = now + (poll_interval / 1000.0)

                wait_seconds = self._compute_wait_seconds(next_tick)
                if wait_seconds > 0:
                    self._wake_event.wait(wait_seconds)
                    self._wake_event.clear()
        except KeyboardInterrupt:
            log("INFO", "symphony_stopping", reason="keyboard_interrupt")
        finally:
            self.stop()

        log("INFO", "symphony_stopped", ticks=tick_count)
        return 0

    def stop(self) -> None:
        if self._stop_event.is_set():
            return
        self._stop_event.set()
        self._wake_event.set()
        for issue_id in list(self._worker_controls):
            self._request_stop(issue_id, cleanup_on_exit=False, reason="shutdown")

        for control in self._worker_controls.values():
            control.thread.join(timeout=2.0)
        self._worker_controls.clear()

        if self._server is not None:
            self._server.stop()

    def request_refresh(self) -> bool:
        was_set = self._refresh_event.is_set()
        self._refresh_event.set()
        self._wake_event.set()
        with self._state_lock:
            self._record_manual_action_locked(
                action="refresh_cycle",
                issue_identifier=None,
                ok=True,
                status="queued" if not was_set else "coalesced",
                message="Refresh cycle requested from dashboard.",
            )
        return not was_set

    def request_retry_now(self, issue_identifier: str) -> dict[str, Any]:
        identifier_norm = issue_identifier.strip()
        if not identifier_norm:
            with self._state_lock:
                self._record_manual_action_locked(
                    action="retry_now",
                    issue_identifier=None,
                    ok=False,
                    status="error",
                    message="issue_identifier is required.",
                )
            return {
                "ok": False,
                "error": "missing_issue_identifier",
                "message": "issue_identifier is required.",
            }

        now_mono = time.monotonic()
        orphan_issue_id: str | None = None
        orphan_issue_claimed = False
        orphan_issue_has_last_error = False
        with self._state_lock:
            for issue_id, entry in self._state.retry_attempts.items():
                if entry.identifier != identifier_norm:
                    continue
                entry.due_at_monotonic = now_mono
                self._state.profiling.incr("manual_retry_now_requests")
                self._wake_event.set()
                self._record_manual_action_locked(
                    action="retry_now",
                    issue_identifier=identifier_norm,
                    ok=True,
                    status="retry_due_now",
                    message="Retry due time moved to now.",
                )
                return {
                    "ok": True,
                    "queued": True,
                    "issue_id": issue_id,
                    "issue_identifier": identifier_norm,
                    "status": "retrying",
                    "message": "Retry due time moved to now.",
                }
            for issue_id, mapped_identifier in self._state.issue_identifiers.items():
                if mapped_identifier != identifier_norm:
                    continue
                orphan_issue_id = issue_id
                orphan_issue_claimed = issue_id in self._state.claimed
                orphan_issue_has_last_error = issue_id in self._state.last_errors
                break

        running_issue_id: str | None = None
        with self._state_lock:
            for issue_id, entry in self._state.running.items():
                if entry.issue_identifier == identifier_norm:
                    running_issue_id = issue_id
                    break

        if running_issue_id is not None:
            self._request_stop(
                running_issue_id, cleanup_on_exit=False, reason="manual_retry"
            )
            with self._state_lock:
                self._state.profiling.incr("manual_retry_now_requests")
                self._record_manual_action_locked(
                    action="retry_now",
                    issue_identifier=identifier_norm,
                    ok=True,
                    status="stop_requested",
                    message="Stop requested; retry will be scheduled on worker exit.",
                )
            return {
                "ok": True,
                "queued": True,
                "issue_id": running_issue_id,
                "issue_identifier": identifier_norm,
                "status": "running",
                "message": "Stop requested; retry will be scheduled on worker exit.",
            }

        if (
            orphan_issue_id is not None
            and orphan_issue_claimed
            and orphan_issue_has_last_error
        ):
            self._schedule_retry(
                orphan_issue_id,
                identifier_norm,
                1,
                error="manual_retry_recovered",
                continuation=True,
            )
            with self._state_lock:
                self._state.profiling.incr("manual_retry_now_requests")
                self._record_manual_action_locked(
                    action="retry_now",
                    issue_identifier=identifier_norm,
                    ok=True,
                    status="retry_recovered",
                    message="Recovered orphaned retry state and queued immediate retry.",
                )
            return {
                "ok": True,
                "queued": True,
                "issue_id": orphan_issue_id,
                "issue_identifier": identifier_norm,
                "status": "retrying",
                "message": "Recovered orphaned retry state and queued immediate retry.",
            }

        try:
            candidates = self._tracker.fetch_candidate_issues()
        except TrackerError as exc:
            with self._state_lock:
                self._record_manual_action_locked(
                    action="retry_now",
                    issue_identifier=identifier_norm,
                    ok=False,
                    status="tracker_error",
                    message=f"Unable to fetch candidate issues: {exc}",
                )
            return {
                "ok": False,
                "error": "tracker_error",
                "issue_identifier": identifier_norm,
                "message": f"Unable to fetch candidate issues: {exc}",
            }

        issue_match = next(
            (item for item in candidates if item.identifier == identifier_norm),
            None,
        )
        if issue_match is not None:
            if not self._is_dispatch_eligible(issue_match, from_retry=True):
                with self._state_lock:
                    self._record_manual_action_locked(
                        action="retry_now",
                        issue_identifier=identifier_norm,
                        ok=False,
                        status="not_eligible",
                        message=(
                            "Issue exists but is not dispatch-eligible in the current"
                            " workflow state."
                        ),
                    )
                return {
                    "ok": False,
                    "error": "issue_not_eligible",
                    "issue_identifier": identifier_norm,
                    "message": (
                        "Issue exists but is not dispatch-eligible in the current"
                        " workflow state."
                    ),
                }

            dispatched = self._dispatch_issue(issue_match, attempt=1)
            if not dispatched:
                self._schedule_retry(
                    issue_match.id,
                    issue_match.identifier,
                    1,
                    error="manual_retry_deferred",
                    continuation=True,
                )
                status = "retrying"
                message = "Issue dispatch deferred; queued immediate retry when capacity opens."
            else:
                status = "running"
                message = "Issue dispatch started."

            with self._state_lock:
                self._state.profiling.incr("manual_retry_now_requests")
                self._record_manual_action_locked(
                    action="retry_now",
                    issue_identifier=identifier_norm,
                    ok=True,
                    status=status,
                    message=message,
                )
            return {
                "ok": True,
                "queued": True,
                "issue_id": issue_match.id,
                "issue_identifier": identifier_norm,
                "status": status,
                "message": message,
            }

        with self._state_lock:
            self._record_manual_action_locked(
                action="retry_now",
                issue_identifier=identifier_norm,
                ok=False,
                status="not_found",
                message="Issue is not currently tracked in running/retrying sets.",
            )
        return {
            "ok": False,
            "error": "issue_not_found",
            "issue_identifier": identifier_norm,
            "message": "Issue is not currently tracked in running/retrying sets.",
        }

    def request_stop_now(self, issue_identifier: str) -> dict[str, Any]:
        identifier_norm = issue_identifier.strip()
        if not identifier_norm:
            with self._state_lock:
                self._record_manual_action_locked(
                    action="stop_worker",
                    issue_identifier=None,
                    ok=False,
                    status="error",
                    message="issue_identifier is required.",
                )
            return {
                "ok": False,
                "error": "missing_issue_identifier",
                "message": "issue_identifier is required.",
            }
        target_issue_id: str | None = None
        with self._state_lock:
            for issue_id, entry in self._state.running.items():
                if entry.issue_identifier != identifier_norm:
                    continue
                self._state.profiling.incr("manual_stop_requests")
                target_issue_id = issue_id
                break
        if target_issue_id is not None:
            self._request_stop(
                target_issue_id, cleanup_on_exit=False, reason="manual_stop"
            )
            with self._state_lock:
                self._record_manual_action_locked(
                    action="stop_worker",
                    issue_identifier=identifier_norm,
                    ok=True,
                    status="stop_requested",
                    message="Stop requested for active worker.",
                )
            return {
                "ok": True,
                "queued": True,
                "issue_id": target_issue_id,
                "issue_identifier": identifier_norm,
                "status": "running",
                "message": "Stop requested for active worker.",
            }
        with self._state_lock:
            self._record_manual_action_locked(
                action="stop_worker",
                issue_identifier=identifier_norm,
                ok=False,
                status="not_running",
                message="Issue is not currently running.",
            )
        return {
            "ok": False,
            "error": "issue_not_running",
            "issue_identifier": identifier_norm,
            "message": "Issue is not currently running.",
        }

    def run_dashboard_tool(
        self, tool_name: str, payload: dict[str, Any]
    ) -> dict[str, Any]:
        tool = tool_name.strip().lower()
        issue_identifier = str(payload.get("issue_identifier") or "").strip()
        if tool == "refresh_cycle":
            queued = self.request_refresh()
            return {
                "ok": True,
                "tool": tool,
                "queued": queued,
                "message": "Refresh cycle requested.",
            }
        if tool == "retry_now":
            result = self.request_retry_now(issue_identifier)
            result["tool"] = tool
            return result
        if tool == "stop_worker":
            result = self.request_stop_now(issue_identifier)
            result["tool"] = tool
            return result
        if tool == "inspect_issue":
            if not issue_identifier:
                with self._state_lock:
                    self._record_manual_action_locked(
                        action="inspect_issue",
                        issue_identifier=None,
                        ok=False,
                        status="error",
                        message="issue_identifier is required.",
                    )
                return {
                    "ok": False,
                    "tool": tool,
                    "error": "missing_issue_identifier",
                    "message": "issue_identifier is required.",
                }
            payload_issue = self.snapshot_issue(issue_identifier)
            if payload_issue is None:
                with self._state_lock:
                    self._record_manual_action_locked(
                        action="inspect_issue",
                        issue_identifier=issue_identifier,
                        ok=False,
                        status="not_found",
                        message="Issue is not currently tracked.",
                    )
                return {
                    "ok": False,
                    "tool": tool,
                    "error": "issue_not_found",
                    "message": "Issue is not currently tracked.",
                }
            with self._state_lock:
                self._record_manual_action_locked(
                    action="inspect_issue",
                    issue_identifier=issue_identifier,
                    ok=True,
                    status="ok",
                    message="Issue snapshot returned.",
                )
            return {"ok": True, "tool": tool, "issue": payload_issue}
        with self._state_lock:
            self._record_manual_action_locked(
                action=tool or "unknown_tool",
                issue_identifier=issue_identifier or None,
                ok=False,
                status="unsupported_tool",
                message=f"Unsupported dashboard tool: {tool_name}",
            )
        return {
            "ok": False,
            "tool": tool,
            "error": "unsupported_tool",
            "message": f"Unsupported dashboard tool: {tool_name}",
        }

    def _record_manual_action_locked(
        self,
        *,
        action: str,
        issue_identifier: str | None,
        ok: bool,
        status: str,
        message: str,
    ) -> None:
        self._state.manual_actions.append(
            {
                "at": now_utc().isoformat().replace("+00:00", "Z"),
                "action": action,
                "issue_identifier": issue_identifier,
                "ok": ok,
                "status": status,
                "message": message,
            }
        )
        if len(self._state.manual_actions) > 80:
            del self._state.manual_actions[: len(self._state.manual_actions) - 80]

    def _record_profile_duration(self, label: str, duration_ms: float) -> None:
        with self._state_lock:
            self._state.profiling.observe_latency(label, duration_ms)

    def _set_suspension_locked(
        self,
        *,
        kind: str,
        message: str,
        resume_delay_seconds: int | None,
        auto_resume: bool,
    ) -> None:
        now_mono = time.monotonic()
        resume_at: float | None = None
        if resume_delay_seconds is not None:
            resume_at = now_mono + float(max(resume_delay_seconds, 1))
        self._state.suspension_kind = kind
        self._state.suspension_message = message
        if self._state.suspension_since_utc is None:
            self._state.suspension_since_utc = now_utc()
        self._state.suspension_resume_at_monotonic = resume_at
        self._state.suspension_auto_resume = auto_resume
        self._state.profiling.incr("orchestrator_suspensions")

    def _clear_suspension_locked(self) -> str | None:
        previous_kind = self._state.suspension_kind
        self._state.suspension_kind = None
        self._state.suspension_message = None
        self._state.suspension_since_utc = None
        self._state.suspension_resume_at_monotonic = None
        self._state.suspension_auto_resume = False
        return previous_kind

    def _resume_ready_locked(self, now_mono: float) -> bool:
        if self._state.suspension_kind is None:
            return True
        resume_at = self._state.suspension_resume_at_monotonic
        if resume_at is None:
            return False
        return now_mono >= resume_at

    def _suspension_due_in_locked(self, now_mono: float) -> float | None:
        resume_at = self._state.suspension_resume_at_monotonic
        if resume_at is None:
            return None
        return max(resume_at - now_mono, 0.0)

    def _compute_wait_seconds(self, next_tick_monotonic: float) -> float:
        now = time.monotonic()
        if next_tick_monotonic <= now:
            return 0.0

        wait_seconds = next_tick_monotonic - now
        with self._state_lock:
            next_retry_due = _next_retry_due(self._state.retry_attempts)
            suspension_due = self._suspension_due_in_locked(now)
        if next_retry_due is not None:
            wait_seconds = min(wait_seconds, max(next_retry_due - now, 0.0))
        if suspension_due is not None:
            wait_seconds = min(wait_seconds, suspension_due)

        return _clamp_wait(wait_seconds, minimum=0.0, maximum=5.0)

    def snapshot_state(self) -> dict[str, Any]:
        with self._state_lock:
            now = datetime.now(UTC)
            now_mono = time.monotonic()
            usage = _sample_process_usage()
            if usage is not None:
                self._state.profiling.observe_resource_usage(
                    cpu_user_s=usage["cpu_user_s"],
                    cpu_system_s=usage["cpu_system_s"],
                    rss_high_water_kb=int(usage["rss_high_water_kb"]),
                )
            running_rows = []
            agent_panes = []
            active_seconds = 0.0
            recent_events: list[dict[str, str | None]] = []
            for issue_id, entry in self._state.running.items():
                active_seconds += max(now_mono - entry.started_at_monotonic, 0.0)
                session = entry.session
                last_event_at = (
                    session.last_codex_timestamp.isoformat().replace("+00:00", "Z")
                    if session.last_codex_timestamp
                    else None
                )
                last_turn_duration_ms = (
                    round(session.last_turn_duration_ms, 3)
                    if session.last_turn_duration_ms is not None
                    else None
                )
                dispatch_to_first_event_ms = (
                    round(entry.dispatch_to_first_event_ms, 3)
                    if entry.dispatch_to_first_event_ms is not None
                    else None
                )
                running_rows.append(
                    {
                        "issue_id": issue_id,
                        "issue_identifier": entry.issue_identifier,
                        "state": entry.issue.state,
                        "worker_role": entry.worker_role,
                        "worker_name": entry.worker_name,
                        "session_id": session.session_id,
                        "turn_count": session.turn_count,
                        "last_event": session.last_codex_event,
                        "last_message": session.last_codex_message,
                        "started_at": entry.started_at_utc.isoformat().replace(
                            "+00:00", "Z"
                        ),
                        "last_event_at": last_event_at,
                        "last_turn_duration_ms": last_turn_duration_ms,
                        "max_turn_duration_ms": round(session.max_turn_duration_ms, 3),
                        "dispatch_to_first_event_ms": dispatch_to_first_event_ms,
                        "tokens": {
                            "input_tokens": session.codex_input_tokens,
                            "output_tokens": session.codex_output_tokens,
                            "total_tokens": session.codex_total_tokens,
                        },
                        "url": entry.issue.url,
                    }
                )
                agent_panes.append(
                    {
                        "pane_id": (
                            session.session_id
                            or entry.issue_identifier
                            or entry.worker_name
                            or issue_id
                        ),
                        "agent_name": entry.worker_name,
                        "worker_name": entry.worker_name,
                        "role": entry.worker_role,
                        "issue_id": issue_id,
                        "issue_identifier": entry.issue_identifier,
                        "state": entry.issue.state,
                        "status": entry.issue.state,
                        "session_id": session.session_id,
                        "turn_count": session.turn_count,
                        "started_at": entry.started_at_utc.isoformat().replace(
                            "+00:00", "Z"
                        ),
                        "last_event": session.last_codex_event,
                        "last_message": session.last_codex_message,
                        "last_event_at": last_event_at,
                        "last_turn_duration_ms": last_turn_duration_ms,
                        "max_turn_duration_ms": round(session.max_turn_duration_ms, 3),
                        "dispatch_to_first_event_ms": dispatch_to_first_event_ms,
                        "tokens": {
                            "input_tokens": session.codex_input_tokens,
                            "output_tokens": session.codex_output_tokens,
                            "total_tokens": session.codex_total_tokens,
                        },
                        "recent_events": list(session.recent_events[-20:]),
                    }
                )
                for event in session.recent_events[-3:]:
                    recent_events.append(
                        {
                            "issue_id": issue_id,
                            "issue_identifier": entry.issue_identifier,
                            "at": event.get("at"),
                            "event": event.get("event"),
                            "message": event.get("message"),
                            "detail": event.get("detail"),
                        }
                    )

            retry_rows = []
            for entry in self._state.retry_attempts.values():
                due_seconds = max(entry.due_at_monotonic - now_mono, 0.0)
                due_at = now + timedelta(seconds=due_seconds)
                retry_rows.append(
                    {
                        "issue_id": entry.issue_id,
                        "issue_identifier": entry.identifier,
                        "attempt": entry.attempt,
                        "due_at": due_at.isoformat().replace("+00:00", "Z"),
                        "due_in_seconds": round(due_seconds, 3),
                        "error": entry.error,
                    }
                )

            retry_id_to_identifier = {
                row["issue_id"]: row["issue_identifier"] for row in retry_rows
            }
            running_id_to_identifier = {
                row["issue_id"]: row["issue_identifier"] for row in running_rows
            }
            attention = []
            for row in retry_rows:
                if row["error"]:
                    attention.append(
                        {
                            "issue_id": row["issue_id"],
                            "issue_identifier": row["issue_identifier"],
                            "kind": "retry_error",
                            "message": str(row["error"]),
                            "suggested_action": "Inspect issue logs and decide whether to unblock, edit issue scope, or retry with workflow changes.",
                        }
                    )
            for issue_id, error in self._state.last_errors.items():
                identifier = (
                    running_id_to_identifier.get(issue_id)
                    or retry_id_to_identifier.get(issue_id)
                    or self._state.issue_identifiers.get(issue_id)
                    or issue_id
                )
                attention.append(
                    {
                        "issue_id": issue_id,
                        "issue_identifier": identifier,
                        "kind": "last_error",
                        "message": error,
                        "suggested_action": "Review failure context and provide human guidance if the agent is blocked.",
                    }
                )
            unique_attention: list[dict[str, str]] = []
            seen_attention: set[tuple[str, str, str]] = set()
            for item in attention:
                key = (
                    str(item["issue_id"]),
                    str(item["kind"]),
                    str(item["message"]),
                )
                if key in seen_attention:
                    continue
                seen_attention.add(key)
                unique_attention.append(item)
            suspension_payload = None
            if self._state.suspension_kind is not None:
                due_in = self._suspension_due_in_locked(now_mono)
                suspension_payload = {
                    "active": True,
                    "kind": self._state.suspension_kind,
                    "message": self._state.suspension_message,
                    "auto_resume": self._state.suspension_auto_resume,
                    "due_in_seconds": (
                        round(due_in, 3) if due_in is not None else None
                    ),
                    "since": (
                        self._state.suspension_since_utc.isoformat().replace(
                            "+00:00", "Z"
                        )
                        if self._state.suspension_since_utc is not None
                        else None
                    ),
                }
                unique_attention.insert(
                    0,
                    {
                        "issue_id": "system",
                        "issue_identifier": "SYSTEM",
                        "kind": self._state.suspension_kind,
                        "message": str(self._state.suspension_message or ""),
                        "suggested_action": (
                            "If this is an auth pause, run codex authentication and wait"
                            " for auto-resume. If this is a rate-limit pause, Symphony"
                            " will resume automatically when quota returns."
                        ),
                    },
                )

            recent_events.sort(key=lambda row: str(row.get("at") or ""), reverse=True)
            recent_events = recent_events[:30]
            agent_panes.sort(
                key=lambda row: str(row.get("last_event_at") or ""), reverse=True
            )
            profiling_snapshot = self._state.profiling.snapshot(now_monotonic=now_mono)
            totals_snapshot = self._state.codex_totals.snapshot(active_seconds)

            return {
                "generated_at": now.isoformat().replace("+00:00", "Z"),
                "counts": {
                    "running": len(running_rows),
                    "retrying": len(retry_rows),
                    "completed": len(self._state.completed),
                    "claimed": len(self._state.claimed),
                },
                "running": running_rows,
                "agent_panes": agent_panes,
                "retrying": retry_rows,
                "codex_totals": totals_snapshot,
                "tokens_per_second": totals_snapshot.get("tokens_per_second", 0.0),
                "rate_limits": self._state.codex_rate_limits,
                "recent_events": recent_events,
                "manual_actions": list(self._state.manual_actions[-40:]),
                "attention": unique_attention,
                "needs_human_attention": bool(unique_attention),
                "suspension": suspension_payload,
                "profiling": profiling_snapshot,
                "runtime": {
                    "exec_mode": self._exec_mode,
                    "codex_command": self._config.codex.command,
                    "default_role": self._config.agent.default_role,
                    "role_pools": dict(self._config.agent.role_pools),
                },
            }

    def snapshot_issue(self, issue_identifier: str) -> dict[str, Any] | None:
        identifier_norm = issue_identifier.strip()
        with self._state_lock:
            for issue_id, entry in self._state.running.items():
                if entry.issue_identifier == identifier_norm:
                    session = entry.session
                    return {
                        "issue_identifier": identifier_norm,
                        "issue_id": issue_id,
                        "status": "running",
                        "workspace": {
                            "path": str(
                                self._workspace.root
                                / sanitize_workspace_key(entry.issue_identifier)
                            )
                        },
                        "attempts": {
                            "restart_count": entry.retry_attempt,
                            "current_retry_attempt": entry.retry_attempt,
                        },
                        "running": {
                            "session_id": session.session_id,
                            "turn_count": session.turn_count,
                            "state": entry.issue.state,
                            "worker_role": entry.worker_role,
                            "worker_name": entry.worker_name,
                            "started_at": entry.started_at_utc.isoformat().replace(
                                "+00:00", "Z"
                            ),
                            "last_event": session.last_codex_event,
                            "last_message": session.last_codex_message,
                            "last_event_at": (
                                session.last_codex_timestamp.isoformat().replace(
                                    "+00:00", "Z"
                                )
                                if session.last_codex_timestamp
                                else None
                            ),
                            "last_turn_duration_ms": session.last_turn_duration_ms,
                            "max_turn_duration_ms": session.max_turn_duration_ms,
                            "dispatch_to_first_event_ms": (
                                entry.dispatch_to_first_event_ms
                            ),
                            "tokens": {
                                "input_tokens": session.codex_input_tokens,
                                "output_tokens": session.codex_output_tokens,
                                "total_tokens": session.codex_total_tokens,
                            },
                        },
                        "logs": {
                            "codex_session_logs": [],
                        },
                        "recent_events": list(session.recent_events),
                        "tracked": {},
                        "retry": None,
                        "last_error": self._state.last_errors.get(issue_id),
                    }

            for entry in self._state.retry_attempts.values():
                if entry.identifier == identifier_norm:
                    return {
                        "issue_identifier": identifier_norm,
                        "issue_id": entry.issue_id,
                        "status": "retrying",
                        "attempts": {
                            "restart_count": entry.attempt,
                            "current_retry_attempt": entry.attempt,
                        },
                        "running": None,
                        "retry": {
                            "attempt": entry.attempt,
                            "due_in_seconds": round(
                                max(entry.due_at_monotonic - time.monotonic(), 0.0), 3
                            ),
                            "error": entry.error,
                        },
                        "logs": {
                            "codex_session_logs": [],
                        },
                        "recent_events": [],
                        "tracked": {},
                        "last_error": self._state.last_errors.get(entry.issue_id),
                    }

            issue_id_from_index = None
            for issue_id, identifier in self._state.issue_identifiers.items():
                if identifier == identifier_norm:
                    issue_id_from_index = issue_id
                    break
            if issue_id_from_index is not None and (
                issue_id_from_index in self._state.last_errors
                or issue_id_from_index in self._state.claimed
            ):
                return {
                    "issue_identifier": identifier_norm,
                    "issue_id": issue_id_from_index,
                    "status": "blocked",
                    "attempts": {
                        "restart_count": 0,
                        "current_retry_attempt": None,
                    },
                    "running": None,
                    "retry": None,
                    "logs": {
                        "codex_session_logs": [],
                    },
                    "recent_events": [],
                    "tracked": {
                        "claimed": issue_id_from_index in self._state.claimed,
                        "running": issue_id_from_index in self._state.running,
                        "retrying": issue_id_from_index in self._state.retry_attempts,
                    },
                    "last_error": self._state.last_errors.get(issue_id_from_index),
                }
        return None

    def _start_http_server_if_enabled(self) -> None:
        port = self._port_override
        if port is None:
            port = self._config.server.port
        if port is None:
            return
        self._server = DashboardServer(provider=self, port=port)
        bound_port = self._server.start()
        log("INFO", "http_server_started", port=bound_port)

    @profiled("tick")
    def _tick(self) -> None:
        self._reload_workflow_if_changed()
        self._reconcile_running_issues()

        try:
            validate_dispatch_config(self._config)
        except ConfigValidationError as exc:
            log("ERROR", "dispatch_preflight_failed", error=str(exc))
            return

        with self._state_lock:
            now_mono = time.monotonic()
            if not self._resume_ready_locked(now_mono):
                self._state.profiling.incr("tick_suspended")
                return
            resumed_kind = self._clear_suspension_locked()
        if resumed_kind is not None:
            log("INFO", "orchestrator_resumed", reason=resumed_kind)

        try:
            candidates = self._tracker.fetch_candidate_issues()
        except TrackerError as exc:
            log("ERROR", "candidate_fetch_failed", error=str(exc))
            return

        sorted_candidates = sorted(candidates, key=_dispatch_sort_key)
        for issue in sorted_candidates:
            if not self._has_available_global_slot():
                break
            if not self._is_dispatch_eligible(issue, from_retry=False):
                continue
            self._dispatch_issue(issue, attempt=None)

    @profiled("process_due_retries")
    def _process_due_retries(self) -> None:
        now_mono = time.monotonic()
        due: list[RetryEntry] = []
        with self._state_lock:
            if not self._resume_ready_locked(now_mono):
                self._state.profiling.incr("retry_processing_suspended")
                return
            for issue_id, entry in list(self._state.retry_attempts.items()):
                if entry.due_at_monotonic <= now_mono:
                    due.append(entry)
                    del self._state.retry_attempts[issue_id]

        if not due:
            return

        try:
            candidates = self._tracker.fetch_candidate_issues()
        except TrackerError:
            for retry_entry in due:
                self._schedule_retry(
                    retry_entry.issue_id,
                    retry_entry.identifier,
                    retry_entry.attempt + 1,
                    error="retry poll failed",
                    continuation=False,
                )
            return

        by_id = {item.id: item for item in candidates}
        for retry_entry in due:
            self._handle_retry_timer(retry_entry, by_id)

    def _handle_retry_timer(
        self, retry_entry: RetryEntry, by_id: dict[str, Issue]
    ) -> None:
        issue = by_id.get(retry_entry.issue_id)
        if issue is None:
            self._release_claim(retry_entry.issue_id)
            return

        if not self._has_available_global_slot():
            self._schedule_retry(
                issue.id,
                issue.identifier,
                retry_entry.attempt + 1,
                error="no available orchestrator slots",
                continuation=False,
            )
            return

        if not self._is_dispatch_eligible(issue, from_retry=True):
            self._release_claim(retry_entry.issue_id)
            return

        dispatched = self._dispatch_issue(issue, attempt=retry_entry.attempt)
        if dispatched:
            return

        self._schedule_retry(
            issue.id,
            issue.identifier,
            max(retry_entry.attempt, 1),
            error="dispatch_deferred",
            continuation=True,
        )

    @profiled("reconcile_running_issues")
    def _reconcile_running_issues(self) -> None:
        if self._config.codex.stall_timeout_ms > 0:
            self._reconcile_stalled_runs()

        with self._state_lock:
            running_ids = list(self._state.running.keys())
        if not running_ids:
            return

        try:
            refreshed = self._tracker.fetch_issue_states_by_ids(running_ids)
        except TrackerError as exc:
            log("WARNING", "running_state_refresh_failed", error=str(exc))
            return

        for issue_id in running_ids:
            updated = refreshed.get(issue_id)
            if updated is None:
                continue
            state_norm = normalize_state_name(updated.state)
            if state_norm in self._terminal_states:
                self._request_stop(issue_id, cleanup_on_exit=True, reason="terminal")
            elif state_norm in self._active_states:
                with self._state_lock:
                    entry = self._state.running.get(issue_id)
                    if entry is not None:
                        entry.issue = updated
            else:
                self._request_stop(issue_id, cleanup_on_exit=False, reason="non_active")

    def _reconcile_stalled_runs(self) -> None:
        timeout_ms = self._config.codex.stall_timeout_ms
        now = now_utc()

        with self._state_lock:
            snapshot = list(self._state.running.items())

        for issue_id, entry in snapshot:
            last = entry.session.last_codex_timestamp
            if last is None:
                elapsed_ms = max(
                    (time.monotonic() - entry.started_at_monotonic) * 1000.0, 0.0
                )
            else:
                elapsed_ms = max((now - last).total_seconds() * 1000.0, 0.0)
            if elapsed_ms > timeout_ms:
                log(
                    "WARNING",
                    "stalled_run_detected",
                    issue_id=issue_id,
                    issue_identifier=entry.issue_identifier,
                    elapsed_ms=round(elapsed_ms, 1),
                )
                self._request_stop(issue_id, cleanup_on_exit=False, reason="stall")

    def _dispatch_issue(self, issue: Issue, attempt: int | None) -> bool:
        worker_role = self._derive_worker_role(issue)
        with self._state_lock:
            if issue.id in self._state.running:
                return False
            if attempt is None and issue.id in self._state.claimed:
                return False
            if not self._has_role_capacity_locked(worker_role):
                self._state.profiling.incr("dispatch_deferred_role_pool_full")
                return False

            retry_attempt = 0 if attempt is None else max(attempt, 1)
            running_entry = RunningEntry(
                issue=issue,
                issue_identifier=issue.identifier,
                worker_name=f"symphony-{worker_role}-{issue.identifier}",
                worker_role=worker_role,
                started_at_utc=now_utc(),
                started_at_monotonic=time.monotonic(),
                retry_attempt=retry_attempt,
            )
            self._state.running[issue.id] = running_entry
            self._state.claimed.add(issue.id)
            self._state.retry_attempts.pop(issue.id, None)
            self._state.issue_identifiers[issue.id] = issue.identifier
            self._state.last_errors.pop(issue.id, None)
            self._state.profiling.incr("issues_dispatched")
            if attempt is not None:
                self._state.profiling.incr("issues_dispatched_from_retry")

        stop_event = threading.Event()
        thread = threading.Thread(
            target=self._worker_entry,
            args=(issue, attempt, stop_event),
            daemon=True,
            name=f"symphony-worker-{worker_role}-{issue.identifier}",
        )
        self._worker_controls[issue.id] = WorkerControl(
            thread=thread, stop_event=stop_event
        )
        try:
            thread.start()
        except Exception as exc:  # pragma: no cover - defensive
            with self._state_lock:
                self._state.running.pop(issue.id, None)
                self._state.claimed.discard(issue.id)
                self._state.profiling.incr("dispatch_thread_start_failed")
            self._worker_controls.pop(issue.id, None)
            self._schedule_retry(
                issue.id,
                issue.identifier,
                max(retry_attempt, 1),
                error=f"dispatch_start_failed:{exc.__class__.__name__}:{exc}",
                continuation=False,
            )
            log(
                "ERROR",
                "issue_dispatch_start_failed",
                issue_id=issue.id,
                issue_identifier=issue.identifier,
                error=str(exc),
            )
            return False
        log(
            "INFO",
            "issue_dispatched",
            issue_id=issue.id,
            issue_identifier=issue.identifier,
            worker_role=worker_role,
        )
        return True

    def _derive_worker_role(self, issue: Issue) -> str:
        for label in issue.labels:
            normalized = label.strip().lower()
            for prefix in ("role:", "swarm:"):
                if normalized.startswith(prefix):
                    candidate = normalized.removeprefix(prefix).strip()
                    role = "".join(
                        ch for ch in candidate if ch.isalnum() or ch in {"-", "_"}
                    )
                    if role:
                        return role
        return self._config.agent.default_role

    def _has_role_capacity_locked(self, role: str) -> bool:
        pool_limit = self._config.agent.role_pools.get(role)
        if pool_limit is None:
            return True
        running_count = 0
        for entry in self._state.running.values():
            if entry.worker_role == role:
                running_count += 1
        return running_count < pool_limit

    def _worker_entry(
        self, issue: Issue, attempt: int | None, stop_event: threading.Event
    ) -> None:
        start_mono = time.monotonic()
        workspace_path = None
        final_issue = issue
        with self._state_lock:
            self._state.profiling.incr("workers_started")

        try:
            workspace = self._workspace.create_for_issue(issue.identifier)
            workspace_path = workspace.path
            self._workspace.ensure_workspace_cwd(workspace.path)
            self._workspace.run_before_run(workspace.path)

            first_prompt = self._build_first_turn_prompt(issue, attempt)

            def event_callback(payload: dict[str, Any]) -> None:
                self._event_queue.put(
                    ("codex_update", {"issue_id": issue.id, "payload": payload})
                )
                self._wake_event.set()

            def tool_handler(
                name: str, tool_input: dict[str, Any] | str | None
            ) -> dict[str, Any]:
                if name == "linear_graphql":
                    query: str
                    variables: dict[str, Any] | None
                    if isinstance(tool_input, str):
                        query = tool_input
                        variables = None
                    elif isinstance(tool_input, dict):
                        query = str(tool_input.get("query") or "")
                        vars_raw = tool_input.get("variables")
                        variables = vars_raw if isinstance(vars_raw, dict) else None
                    else:
                        return {"success": False, "error": "invalid_tool_input"}

                    result = self._tracker.execute_raw_graphql(
                        query=query, variables=variables
                    )
                    return {"success": result.success, **result.payload}
                if name == "molt_code_search":
                    return self._tool_code_search(workspace.path, tool_input)
                if name == "molt_cli":
                    return self._tool_molt_cli(workspace.path, tool_input)
                if name == "molt_formal_check":
                    return self._tool_formal_check(workspace.path, tool_input)
                if name == "symphony_state":
                    return self._tool_symphony_state(tool_input)
                return {"success": False, "error": "unsupported_tool_call"}

            client = CodexAppServerClient(
                self._config.codex,
                workspace.path,
                stop_event,
                event_callback=event_callback,
                tool_handler=tool_handler,
            )
            client.start()

            turn_number = 1
            max_turns = self._config.agent.max_turns
            try:
                while True:
                    if stop_event.is_set():
                        raise TurnCancelledError(
                            "turn_cancelled orchestrator requested stop"
                        )

                    prompt = (
                        first_prompt
                        if turn_number == 1
                        else _build_continuation_prompt(
                            final_issue, turn_number, max_turns
                        )
                    )
                    client.run_turn(final_issue, prompt)

                    refreshed = self._tracker.fetch_issue_states_by_ids([issue.id])
                    if issue.id not in refreshed:
                        raise TrackerError("issue_state_refresh_missing")
                    final_issue = refreshed[issue.id]

                    if (
                        normalize_state_name(final_issue.state)
                        not in self._active_states
                    ):
                        break
                    if turn_number >= max_turns:
                        break
                    turn_number += 1
            finally:
                client.stop()

            reason = "normal"
            error = None
        except (HookError, TrackerError, ConfigValidationError) as exc:
            reason = "failed"
            error = str(exc)
        except TurnInputRequiredError as exc:
            reason = "turn_input_required"
            error = str(exc)
        except TurnTimeoutError as exc:
            reason = "turn_timeout"
            error = str(exc)
        except TurnCancelledError as exc:
            reason = "turn_cancelled"
            error = str(exc)
        except TurnFailedError as exc:
            reason = "turn_failed"
            error = str(exc)
        except AgentRunnerError as exc:
            reason = "agent_error"
            error = str(exc)
        except SymphonyError as exc:
            reason = "failed"
            error = str(exc)
        except Exception as exc:  # pragma: no cover - defensive
            reason = "failed"
            error = f"unexpected:{exc.__class__.__name__}:{exc}"
        finally:
            after_run_error: str | None = None
            if workspace_path is not None:
                try:
                    self._workspace.run_after_run(workspace_path)
                except Exception as exc:  # pragma: no cover
                    after_run_error = f"after_run_failed:{exc.__class__.__name__}:{exc}"
                    log(
                        "WARNING",
                        "workspace_after_run_failed",
                        issue_id=issue.id,
                        issue_identifier=issue.identifier,
                        error=str(exc),
                    )
            if after_run_error is not None:
                if reason == "normal":
                    reason = "failed"
                    error = after_run_error
                elif not error:
                    error = after_run_error

            self._event_queue.put(
                (
                    "worker_exit",
                    {
                        "issue_id": issue.id,
                        "issue_identifier": issue.identifier,
                        "reason": reason,
                        "error": error,
                        "duration_seconds": max(time.monotonic() - start_mono, 0.0),
                        "final_issue": final_issue,
                    },
                )
            )
            self._wake_event.set()

    @profiled("drain_events")
    def _drain_events(self) -> None:
        while True:
            try:
                event_type, payload = self._event_queue.get_nowait()
            except queue.Empty:
                return
            with self._state_lock:
                self._state.profiling.observe_queue_depth(self._event_queue.qsize())

            event_start = time.perf_counter()
            if event_type == "codex_update":
                self._on_codex_update(payload["issue_id"], payload["payload"])
            elif event_type == "worker_exit":
                self._on_worker_exit(payload)
            duration_ms = max((time.perf_counter() - event_start) * 1000.0, 0.0)
            with self._state_lock:
                self._state.profiling.incr("events_processed")
                self._state.profiling.observe_latency(
                    f"event_handler_{event_type}", duration_ms
                )

    def _on_codex_update(self, issue_id: str, payload: dict[str, Any]) -> None:
        with self._state_lock:
            entry = self._state.running.get(issue_id)
            if entry is None:
                return

            session = entry.session
            event_name = str(payload.get("event") or "other_message")
            self._state.profiling.incr("codex_events_total")
            self._state.profiling.incr(f"codex_event_{event_name}")
            session.last_codex_event = event_name
            message = payload.get("message")
            if message is not None:
                session.last_codex_message = str(message)
            details = payload.get("details")
            detail_map = details if isinstance(details, dict) else {}
            detail_text = detail_map.get("text_preview")
            detail_text_str = (
                str(detail_text).strip() if isinstance(detail_text, str) else None
            )

            session_id = payload.get("session_id")
            if isinstance(session_id, str) and session_id:
                session.session_id = session_id

            thread_id = payload.get("thread_id")
            if isinstance(thread_id, str) and thread_id:
                session.thread_id = thread_id

            turn_id = payload.get("turn_id")
            if isinstance(turn_id, str) and turn_id:
                session.turn_id = turn_id

            now_mono = time.monotonic()
            if event_name == "session_started":
                session.turn_started_at_monotonic = now_mono
                if entry.dispatch_to_first_event_ms is None:
                    dispatch_ms = max(
                        (now_mono - entry.started_at_monotonic) * 1000.0, 0.0
                    )
                    entry.dispatch_to_first_event_ms = dispatch_ms
                    self._state.profiling.observe_latency(
                        "dispatch_to_first_event", dispatch_ms
                    )

            if event_name == "turn_completed":
                session.turn_count += 1
                self._state.codex_totals.turns_completed += 1

            if event_name in {
                "turn_completed",
                "turn_failed",
                "turn_cancelled",
                "turn_input_required",
            }:
                started = session.turn_started_at_monotonic
                if started is not None:
                    turn_ms = max((now_mono - started) * 1000.0, 0.0)
                    session.last_turn_duration_ms = turn_ms
                    session.max_turn_duration_ms = max(
                        session.max_turn_duration_ms, turn_ms
                    )
                    self._state.profiling.observe_latency("turn", turn_ms)
                session.turn_started_at_monotonic = None

            timestamp = payload.get("timestamp")
            if isinstance(timestamp, (int, float)):
                session.last_codex_timestamp = datetime.fromtimestamp(timestamp, tz=UTC)
            else:
                session.last_codex_timestamp = now_utc()
            session.recent_events.append(
                {
                    "at": session.last_codex_timestamp.isoformat().replace(
                        "+00:00", "Z"
                    ),
                    "event": event_name,
                    "message": session.last_codex_message,
                    "detail": detail_text_str,
                }
            )
            if len(session.recent_events) > 30:
                del session.recent_events[:-30]

            usage = payload.get("usage")
            if isinstance(usage, dict):
                self._accumulate_usage(session, usage)

            rate_limits = payload.get("rate_limits")
            if isinstance(rate_limits, dict):
                self._state.codex_rate_limits = rate_limits
                rate_pause = _derive_rate_limit_suspension(
                    rate_limits,
                    default_resume_seconds=self._rate_limit_resume_default_seconds,
                )
                if rate_pause is not None:
                    self._set_suspension_locked(
                        kind="rate_limited",
                        message=rate_pause["message"],
                        resume_delay_seconds=rate_pause["resume_seconds"],
                        auto_resume=True,
                    )
                    self._wake_event.set()
                elif self._state.suspension_kind == "rate_limited":
                    self._clear_suspension_locked()
                    self._wake_event.set()

            if event_name in {
                "session_started",
                "turn_completed",
                "turn_failed",
                "turn_cancelled",
                "turn_input_required",
                "startup_failed",
            }:
                log(
                    "INFO",
                    "codex_event",
                    issue_id=issue_id,
                    issue_identifier=entry.issue_identifier,
                    session_id=session.session_id,
                    event=event_name,
                )

    def _accumulate_usage(self, session: Any, usage: dict[str, Any]) -> None:
        is_delta = bool(usage.get("delta"))
        input_tokens = int(usage.get("input_tokens", 0) or 0)
        output_tokens = int(usage.get("output_tokens", 0) or 0)
        total_tokens = int(usage.get("total_tokens", 0) or 0)
        if total_tokens == 0 and (input_tokens or output_tokens):
            total_tokens = input_tokens + output_tokens

        if is_delta:
            delta_in = max(input_tokens, 0)
            delta_out = max(output_tokens, 0)
            delta_total = max(total_tokens, 0)
            if delta_total == 0 and (delta_in or delta_out):
                delta_total = delta_in + delta_out

            session.codex_input_tokens += delta_in
            session.codex_output_tokens += delta_out
            session.codex_total_tokens += delta_total
            session.last_reported_input_tokens = session.codex_input_tokens
            session.last_reported_output_tokens = session.codex_output_tokens
            session.last_reported_total_tokens = session.codex_total_tokens
        else:
            delta_in = max(input_tokens - session.last_reported_input_tokens, 0)
            delta_out = max(output_tokens - session.last_reported_output_tokens, 0)
            delta_total = max(total_tokens - session.last_reported_total_tokens, 0)

            session.codex_input_tokens = input_tokens
            session.codex_output_tokens = output_tokens
            session.codex_total_tokens = total_tokens
            session.last_reported_input_tokens = input_tokens
            session.last_reported_output_tokens = output_tokens
            session.last_reported_total_tokens = total_tokens

        self._state.codex_totals.input_tokens += delta_in
        self._state.codex_totals.output_tokens += delta_out
        self._state.codex_totals.total_tokens += delta_total

    def _on_worker_exit(self, payload: dict[str, Any]) -> None:
        issue_id = payload["issue_id"]
        reason = payload["reason"]
        error = payload.get("error")
        final_issue = payload.get("final_issue")
        duration = float(payload.get("duration_seconds") or 0.0)

        with self._state_lock:
            running_entry = self._state.running.pop(issue_id, None)
            self._state.codex_totals.ended_seconds_running += duration
            self._state.profiling.observe_latency("issue_cycle", duration * 1000.0)
            self._state.profiling.incr("workers_exited")
            self._state.profiling.incr(f"worker_exit_{reason}")

        self._worker_controls.pop(issue_id, None)

        if running_entry is None:
            return

        if running_entry.cleanup_on_exit:
            try:
                self._workspace.remove_workspace(running_entry.issue_identifier)
            except Exception as exc:  # pragma: no cover
                log(
                    "WARNING",
                    "workspace_cleanup_failed",
                    issue_id=issue_id,
                    issue_identifier=running_entry.issue_identifier,
                    error=str(exc),
                )

        if running_entry.stop_requested and running_entry.cleanup_reason in {
            "terminal",
            "non_active",
            "shutdown",
        }:
            self._release_claim(issue_id)
            log(
                "INFO",
                "worker_stopped_by_reconciliation",
                issue_id=issue_id,
                issue_identifier=running_entry.issue_identifier,
                reason=running_entry.cleanup_reason,
            )
            return

        if (
            running_entry.stop_requested
            and running_entry.cleanup_reason == "manual_retry"
        ):
            self._schedule_retry(
                issue_id,
                running_entry.issue_identifier,
                max(running_entry.retry_attempt, 1),
                error="manual_retry",
                continuation=True,
            )
            log(
                "INFO",
                "worker_stopped_manual_retry",
                issue_id=issue_id,
                issue_identifier=running_entry.issue_identifier,
            )
            return

        if (
            running_entry.stop_requested
            and running_entry.cleanup_reason == "manual_stop"
        ):
            self._schedule_retry(
                issue_id,
                running_entry.issue_identifier,
                max(running_entry.retry_attempt, 1),
                error="manual_stop",
                continuation=True,
            )
            log(
                "INFO",
                "worker_stopped_manual_stop",
                issue_id=issue_id,
                issue_identifier=running_entry.issue_identifier,
            )
            return

        if reason == "turn_input_required":
            auth_message = (
                "Codex interaction requires login or human input. Run "
                "`codex mcp login` (or `codex login`) and Symphony will auto-resume."
            )
            with self._state_lock:
                self._set_suspension_locked(
                    kind="auth_required",
                    message=auth_message,
                    resume_delay_seconds=self._auth_resume_delay_seconds,
                    auto_resume=True,
                )
                self._state.last_errors[issue_id] = auth_message
            self._schedule_retry(
                issue_id,
                running_entry.issue_identifier,
                max(running_entry.retry_attempt + 1, 1),
                error="auth_required",
                continuation=False,
                delay_override_ms=self._auth_resume_delay_seconds * 1000,
            )
            log(
                "WARNING",
                "worker_paused_auth_required",
                issue_id=issue_id,
                issue_identifier=running_entry.issue_identifier,
            )
            return

        if _looks_like_rate_limit_error(error):
            resume_seconds = self._rate_limit_resume_default_seconds
            with self._state_lock:
                self._set_suspension_locked(
                    kind="rate_limited",
                    message=(
                        "Codex rate-limit exhausted. Symphony will pause and retry"
                        " automatically when quota is expected to recover."
                    ),
                    resume_delay_seconds=resume_seconds,
                    auto_resume=True,
                )
            self._schedule_retry(
                issue_id,
                running_entry.issue_identifier,
                max(running_entry.retry_attempt + 1, 1),
                error=error or "rate_limited",
                continuation=False,
                delay_override_ms=resume_seconds * 1000,
            )
            with self._state_lock:
                self._state.last_errors[issue_id] = str(error or "rate_limited")
            log(
                "WARNING",
                "worker_paused_rate_limited",
                issue_id=issue_id,
                issue_identifier=running_entry.issue_identifier,
                reason=reason,
                error=error,
            )
            return

        if reason == "normal":
            with self._state_lock:
                self._state.completed.add(issue_id)
                self._state.last_errors.pop(issue_id, None)
            self._schedule_retry(
                issue_id,
                running_entry.issue_identifier,
                1,
                error=None,
                continuation=True,
            )
            return

        next_attempt = max(running_entry.retry_attempt + 1, 1)
        self._schedule_retry(
            issue_id,
            running_entry.issue_identifier,
            next_attempt,
            error=error or reason,
            continuation=False,
        )
        with self._state_lock:
            self._state.last_errors[issue_id] = error or reason
        log(
            "WARNING",
            "worker_failed_retrying",
            issue_id=issue_id,
            issue_identifier=running_entry.issue_identifier,
            reason=reason,
            error=error,
        )

        if isinstance(final_issue, Issue):
            with self._state_lock:
                current = self._state.running.get(issue_id)
                if current is not None:
                    current.issue = final_issue

    def _release_claim(self, issue_id: str) -> None:
        with self._state_lock:
            self._state.claimed.discard(issue_id)
            self._state.retry_attempts.pop(issue_id, None)

    def _schedule_retry(
        self,
        issue_id: str,
        identifier: str,
        attempt: int,
        *,
        error: str | None,
        continuation: bool,
        delay_override_ms: int | None = None,
    ) -> None:
        if delay_override_ms is not None:
            delay_ms = max(int(delay_override_ms), 1000)
        elif continuation:
            delay_ms = 1000
        else:
            max_backoff = self._config.agent.max_retry_backoff_ms
            delay_ms = min(10000 * (2 ** max(attempt - 1, 0)), max_backoff)

        due_at = time.monotonic() + (delay_ms / 1000.0)
        entry = RetryEntry(
            issue_id=issue_id,
            identifier=identifier,
            attempt=attempt,
            due_at_monotonic=due_at,
            error=error,
        )
        with self._state_lock:
            self._state.retry_attempts[issue_id] = entry
            self._state.claimed.add(issue_id)
            self._state.issue_identifiers[issue_id] = identifier
            self._state.profiling.incr("retries_scheduled")
            self._state.profiling.observe_latency("retry_backoff", float(delay_ms))
        self._wake_event.set()
        log(
            "INFO",
            "retry_scheduled",
            issue_id=issue_id,
            issue_identifier=identifier,
            attempt=attempt,
            delay_ms=delay_ms,
            error=error,
        )

    def _has_available_global_slot(self) -> bool:
        with self._state_lock:
            return len(self._state.running) < self._config.agent.max_concurrent_agents

    def _is_dispatch_eligible(self, issue: Issue, *, from_retry: bool) -> bool:
        if not issue.id or not issue.identifier or not issue.title or not issue.state:
            return False

        state_norm = normalize_state_name(issue.state)
        if state_norm not in self._active_states or state_norm in self._terminal_states:
            return False

        if not blocker_allows_todo(issue, self._terminal_states):
            return False

        with self._state_lock:
            if issue.id in self._state.running:
                return False
            if not from_retry and issue.id in self._state.claimed:
                return False

            state_counts: dict[str, int] = {}
            for running in self._state.running.values():
                key = normalize_state_name(running.issue.state)
                state_counts[key] = state_counts.get(key, 0) + 1

            state_limit = self._config.agent.max_concurrent_agents_by_state.get(
                state_norm,
                self._config.agent.max_concurrent_agents,
            )
            if state_counts.get(state_norm, 0) >= state_limit:
                return False

            if len(self._state.running) >= self._config.agent.max_concurrent_agents:
                return False

        return True

    def _request_stop(self, issue_id: str, cleanup_on_exit: bool, reason: str) -> None:
        control = self._worker_controls.get(issue_id)
        if control is not None:
            control.stop_event.set()
            self._wake_event.set()

        with self._state_lock:
            entry = self._state.running.get(issue_id)
            if entry is None:
                return
            entry.stop_requested = True
            entry.cleanup_on_exit = cleanup_on_exit
            entry.cleanup_reason = reason

    @profiled("reload_workflow")
    def _reload_workflow_if_changed(self) -> None:
        try:
            reloaded = maybe_reload_workflow(self._workflow)
        except (
            MissingWorkflowFileError,
            WorkflowParseError,
            WorkflowFrontMatterNotMapError,
        ) as exc:
            log("ERROR", "workflow_reload_failed", error=str(exc))
            return
        if reloaded is None:
            return

        try:
            config = build_runtime_config(reloaded)
            validate_dispatch_config(config)
        except ConfigValidationError as exc:
            log("ERROR", "workflow_reload_invalid", error=str(exc))
            return

        self._workflow = reloaded
        self._config = config
        self._tracker = LinearTrackerClient(config.tracker)
        self._workspace = WorkspaceManager(config.workspace, config.hooks)
        self._active_states = normalize_state_list(config.tracker.active_states)
        self._terminal_states = normalize_state_list(config.tracker.terminal_states)

        with self._state_lock:
            self._state.poll_interval_ms = config.polling.interval_ms
            self._state.max_concurrent_agents = config.agent.max_concurrent_agents

        log("INFO", "workflow_reloaded", workflow_path=reloaded.path)

    def _startup_terminal_workspace_cleanup(self) -> None:
        try:
            terminal_issues = self._tracker.fetch_issues_by_states(
                list(self._config.tracker.terminal_states)
            )
        except TrackerError as exc:
            log("WARNING", "startup_terminal_cleanup_skipped", error=str(exc))
            return

        for issue in terminal_issues:
            try:
                self._workspace.remove_workspace(issue.identifier)
            except Exception as exc:  # pragma: no cover
                log(
                    "WARNING",
                    "startup_terminal_cleanup_failed",
                    issue_identifier=issue.identifier,
                    error=str(exc),
                )

    def _build_first_turn_prompt(self, issue: Issue, attempt: int | None) -> str:
        template = self._workflow.prompt_template.strip()
        if not template:
            template = "You are working on an issue from Linear."
        issue_payload = _issue_to_template_payload(issue)
        return render_prompt(template, issue=issue_payload, attempt=attempt)

    def _tool_code_search(
        self, workspace_path: Any, tool_input: dict[str, Any] | str | None
    ) -> dict[str, Any]:
        pattern = ""
        paths: list[str] = ["."]
        globs: list[str] = []
        ignore_case = False
        max_output_chars = 12000
        if isinstance(tool_input, str):
            pattern = tool_input.strip()
        elif isinstance(tool_input, dict):
            pattern = str(tool_input.get("pattern") or "").strip()
            raw_paths = tool_input.get("paths")
            if isinstance(raw_paths, list):
                paths = [str(item) for item in raw_paths if str(item).strip()]
            raw_globs = tool_input.get("glob")
            if isinstance(raw_globs, list):
                globs = [str(item) for item in raw_globs if str(item).strip()]
            ignore_case = bool(tool_input.get("ignore_case", False))
            max_output_chars = _coerce_output_limit(tool_input.get("max_output_chars"))
        if not pattern:
            return {"success": False, "error": "missing_pattern"}
        cmd = [
            *_python_runner_args(),
            "tools/code_search.py",
            pattern,
            *paths,
        ]
        for glob in globs:
            cmd.extend(["--glob", glob])
        if ignore_case:
            cmd.append("--ignore-case")
        return _run_tool_command(
            cmd,
            cwd=workspace_path,
            max_output_chars=max_output_chars,
        )

    def _tool_molt_cli(
        self, workspace_path: Any, tool_input: dict[str, Any] | str | None
    ) -> dict[str, Any]:
        args: list[str]
        max_output_chars = 16000
        if isinstance(tool_input, str):
            args = [part for part in tool_input.strip().split(" ") if part]
        elif isinstance(tool_input, dict):
            raw_args = tool_input.get("args")
            if not isinstance(raw_args, list):
                return {"success": False, "error": "invalid_tool_input"}
            args = [str(item) for item in raw_args if str(item).strip()]
            max_output_chars = _coerce_output_limit(tool_input.get("max_output_chars"))
        else:
            return {"success": False, "error": "invalid_tool_input"}
        if not args:
            return {"success": False, "error": "missing_args"}
        subcommand = args[0]
        if subcommand not in {"build", "run", "compare", "check", "test"}:
            return {
                "success": False,
                "error": "unsupported_molt_cli_subcommand",
                "subcommand": subcommand,
            }
        cmd = [*_python_runner_args(), "-m", "molt.cli", *args]
        return _run_tool_command(
            cmd,
            cwd=workspace_path,
            max_output_chars=max_output_chars,
        )

    def _tool_formal_check(
        self, workspace_path: Any, tool_input: dict[str, Any] | str | None
    ) -> dict[str, Any]:
        suite = "quint-run"
        invariant = "Inv"
        spec_path = "formal/quint/molt_build_determinism.qnt"
        max_steps = 10
        max_output_chars = 16000
        if isinstance(tool_input, str):
            suite = tool_input.strip() or suite
        elif isinstance(tool_input, dict):
            suite = str(tool_input.get("suite") or suite).strip()
            invariant = str(tool_input.get("invariant") or invariant).strip() or "Inv"
            spec_path = str(tool_input.get("spec_path") or spec_path).strip()
            max_steps = _coerce_positive_int(tool_input.get("max_steps"), default=10)
            max_output_chars = _coerce_output_limit(tool_input.get("max_output_chars"))
        else:
            return {"success": False, "error": "invalid_tool_input"}

        if suite == "lean":
            return _run_tool_command(
                ["lake", "build"],
                cwd=Path(workspace_path) / "formal" / "lean",
                max_output_chars=max_output_chars,
                timeout_seconds=900,
            )
        if suite == "quint-run":
            return _run_tool_command(
                [
                    "quint",
                    "run",
                    spec_path,
                    f"--invariant={invariant}",
                    f"--max-steps={max_steps}",
                ],
                cwd=workspace_path,
                max_output_chars=max_output_chars,
                timeout_seconds=600,
            )
        if suite == "quint-verify":
            return _run_tool_command(
                [
                    "quint",
                    "verify",
                    spec_path,
                    f"--invariant={invariant}",
                ],
                cwd=workspace_path,
                max_output_chars=max_output_chars,
                timeout_seconds=900,
            )
        return {"success": False, "error": "unsupported_formal_suite", "suite": suite}

    def _tool_symphony_state(
        self, tool_input: dict[str, Any] | str | None
    ) -> dict[str, Any]:
        issue_identifier: str | None = None
        if isinstance(tool_input, str):
            issue_identifier = tool_input.strip() or None
        elif isinstance(tool_input, dict):
            issue_identifier = (
                str(tool_input.get("issue_identifier") or "").strip() or None
            )
        if issue_identifier:
            payload = self.snapshot_issue(issue_identifier)
            if payload is None:
                return {
                    "success": False,
                    "error": "issue_not_found",
                    "issue_identifier": issue_identifier,
                }
            return {"success": True, "state": payload}
        return {"success": True, "state": self.snapshot_state()}


def create_orchestrator(
    workflow_path: str | None,
    *,
    port_override: int | None = None,
    run_once: bool = False,
) -> SymphonyOrchestrator:
    workflow = load_workflow(_path_from_string(workflow_path))
    config = build_runtime_config(workflow)
    validate_dispatch_config(config)
    return SymphonyOrchestrator(
        workflow,
        config,
        port_override=port_override,
        run_once=run_once,
    )


def _path_from_string(path: str | None):
    from .workflow import discover_workflow_path

    return discover_workflow_path(path)


def _build_continuation_prompt(issue: Issue, turn_number: int, max_turns: int) -> str:
    return (
        f"Continue work on {issue.identifier}: {issue.title}. "
        f"This is continuation turn {turn_number} of {max_turns}. "
        "Do not repeat already completed actions; continue from current repo state, "
        "validate changes, and report concrete outcomes."
    )


def _issue_to_template_payload(issue: Issue) -> dict[str, Any]:
    return {
        "id": issue.id,
        "identifier": issue.identifier,
        "title": issue.title,
        "description": issue.description,
        "priority": issue.priority,
        "state": issue.state,
        "branch_name": issue.branch_name,
        "url": issue.url,
        "labels": list(issue.labels),
        "blocked_by": [
            {
                "id": blocker.id,
                "identifier": blocker.identifier,
                "state": blocker.state,
            }
            for blocker in issue.blocked_by
        ],
        "created_at": (
            issue.created_at.isoformat().replace("+00:00", "Z")
            if issue.created_at
            else None
        ),
        "updated_at": (
            issue.updated_at.isoformat().replace("+00:00", "Z")
            if issue.updated_at
            else None
        ),
    }


def _sample_process_usage() -> dict[str, float | int] | None:
    if resource is None:  # pragma: no cover - platform dependent
        return None
    usage = resource.getrusage(resource.RUSAGE_SELF)
    rss_raw = int(usage.ru_maxrss)
    if sys.platform == "darwin":
        rss_kb = int(rss_raw / 1024)
    else:
        rss_kb = rss_raw
    return {
        "cpu_user_s": float(usage.ru_utime),
        "cpu_system_s": float(usage.ru_stime),
        "rss_high_water_kb": rss_kb,
    }


def _coerce_output_limit(value: Any, *, default: int = 12000) -> int:
    try:
        parsed = int(value)
    except (TypeError, ValueError):
        return default
    return max(2000, min(parsed, 120000))


def _coerce_positive_int(value: Any, *, default: int) -> int:
    try:
        parsed = int(value)
    except (TypeError, ValueError):
        return default
    if parsed <= 0:
        return default
    return parsed


def _python_runner_args() -> list[str]:
    return ["uv", "run", "--python", "3.12", "python3"]


def _run_tool_command(
    cmd: list[str], *, cwd: Any, max_output_chars: int, timeout_seconds: int = 300
) -> dict[str, Any]:
    try:
        proc = subprocess.run(
            cmd,
            cwd=cwd,
            check=False,
            capture_output=True,
            text=True,
            timeout=timeout_seconds,
        )
    except FileNotFoundError as exc:
        return {
            "success": False,
            "error": "tool_command_not_found",
            "message": str(exc),
        }
    except subprocess.TimeoutExpired as exc:
        stdout = (exc.stdout or "")[:max_output_chars]
        stderr = (exc.stderr or "")[:max_output_chars]
        return {
            "success": False,
            "error": "tool_timeout",
            "timeout_seconds": timeout_seconds,
            "stdout": stdout,
            "stderr": stderr,
        }
    stdout = (proc.stdout or "")[:max_output_chars]
    stderr = (proc.stderr or "")[:max_output_chars]
    return {
        "success": proc.returncode == 0,
        "returncode": int(proc.returncode),
        "stdout": stdout,
        "stderr": stderr,
    }


def _derive_rate_limit_suspension(
    rate_limits: dict[str, Any], *, default_resume_seconds: int
) -> dict[str, Any] | None:
    has_credits = _find_bool_key_recursive(
        rate_limits,
        {"hascredits", "has_credits"},
    )
    if has_credits is False:
        resume_seconds = max(default_resume_seconds, 3600)
        return {
            "resume_seconds": resume_seconds,
            "message": (
                "Provider credits are exhausted. Symphony will pause and auto-resume in "
                f"about {_format_duration_brief(resume_seconds)}."
            ),
        }

    exhausted_windows: list[int] = []

    def walk(node: Any) -> None:
        if isinstance(node, dict):
            lowered = {str(key).lower(): value for key, value in node.items()}
            used_percent = _extract_first_number(
                lowered,
                ("usedpercent", "used_percent"),
            )
            remaining = _extract_first_number(
                lowered,
                ("remaining", "remainingrequests", "remainingtokens"),
            )
            exhausted = False
            if used_percent is not None and used_percent >= 99.9:
                exhausted = True
            if remaining is not None and remaining <= 0:
                exhausted = True
            if exhausted:
                window_seconds = _extract_window_seconds(lowered)
                if window_seconds is None:
                    window_seconds = default_resume_seconds
                exhausted_windows.append(window_seconds)
            for value in node.values():
                walk(value)
            return
        if isinstance(node, list):
            for child in node:
                walk(child)

    walk(rate_limits)
    if not exhausted_windows:
        return None
    resume_seconds = max(min(exhausted_windows), 60)
    return {
        "resume_seconds": resume_seconds,
        "message": (
            "Provider rate-limit window is exhausted. Symphony will pause and auto-resume in "
            f"about {_format_duration_brief(resume_seconds)}."
        ),
    }


def _find_bool_key_recursive(value: Any, keys: set[str]) -> bool | None:
    if isinstance(value, dict):
        for key, child in value.items():
            if str(key).lower() in keys and isinstance(child, bool):
                return child
        for child in value.values():
            result = _find_bool_key_recursive(child, keys)
            if result is not None:
                return result
    elif isinstance(value, list):
        for child in value:
            result = _find_bool_key_recursive(child, keys)
            if result is not None:
                return result
    return None


def _extract_first_number(
    candidate: dict[str, Any], keys: tuple[str, ...]
) -> float | None:
    for key in keys:
        if key in candidate:
            value = candidate.get(key)
            if isinstance(value, (int, float)):
                return float(value)
    return None


def _extract_window_seconds(candidate: dict[str, Any]) -> int | None:
    window_seconds = _extract_first_number(
        candidate,
        (
            "windowseconds",
            "windowdurationseconds",
            "resetinseconds",
            "retryafterseconds",
            "retry_after_seconds",
        ),
    )
    if window_seconds is not None and window_seconds > 0:
        return int(window_seconds)

    window_raw = _extract_first_number(
        candidate,
        (
            "windowduration",
            "window_ms",
            "windowmilliseconds",
            "resetinms",
            "retryafterms",
            "retry_after_ms",
        ),
    )
    if window_raw is None or window_raw <= 0:
        return None
    if window_raw >= 100000:
        return int(window_raw / 1000.0)
    return int(window_raw)


def _format_duration_brief(seconds: int) -> str:
    if seconds < 60:
        return f"{seconds}s"
    if seconds < 3600:
        return f"{max(int(seconds / 60), 1)}m"
    if seconds < 86400:
        return f"{max(int(seconds / 3600), 1)}h"
    return f"{max(int(seconds / 86400), 1)}d"


def _looks_like_rate_limit_error(error: Any) -> bool:
    if error is None:
        return False
    text = str(error).strip().lower()
    if not text:
        return False
    if "429" in text:
        return True
    if "rate limit" in text or "ratelimit" in text:
        return True
    if "quota" in text:
        return True
    if "credit" in text and ("exhaust" in text or "deplet" in text):
        return True
    return False


def _dispatch_sort_key(issue: Issue) -> tuple[int, float, str]:
    priority = issue.priority if issue.priority is not None else 999
    created = (
        issue.created_at.timestamp() if issue.created_at is not None else float("inf")
    )
    return (priority, created, issue.identifier)


def _next_retry_due(retry_attempts: dict[str, RetryEntry]) -> float | None:
    if not retry_attempts:
        return None
    return min(entry.due_at_monotonic for entry in retry_attempts.values())


def _clamp_wait(value: float, *, minimum: float, maximum: float) -> float:
    return max(min(value, maximum), minimum)
