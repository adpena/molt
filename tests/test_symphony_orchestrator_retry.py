from __future__ import annotations

import threading
from datetime import UTC, datetime
from types import SimpleNamespace

from molt.symphony.models import (
    Issue,
    OrchestratorState,
    RetryEntry,
    RunningEntry,
    now_utc,
)
from molt.symphony.orchestrator import (
    SymphonyOrchestrator,
    _derive_rate_limit_suspension,
)


def _issue(identifier: str = "MOL-7", issue_id: str = "issue-7") -> Issue:
    return Issue(
        id=issue_id,
        identifier=identifier,
        title="Retry regression",
        description=None,
        priority=None,
        state="Todo",
        branch_name=None,
        url="https://linear.app/moltlang/issue/MOL-7",
        labels=(),
        blocked_by=(),
        created_at=datetime.now(UTC),
        updated_at=datetime.now(UTC),
    )


def _orchestrator_stub() -> SymphonyOrchestrator:
    orchestrator = object.__new__(SymphonyOrchestrator)
    orchestrator._state = OrchestratorState(
        poll_interval_ms=1000, max_concurrent_agents=4
    )
    orchestrator._state_lock = threading.Lock()
    orchestrator._wake_event = threading.Event()
    orchestrator._config = SimpleNamespace(
        codex=SimpleNamespace(command="codex"),
        agent=SimpleNamespace(default_role="executor", role_pools={}),
    )
    orchestrator._worker_controls = {}
    orchestrator._auth_resume_delay_seconds = 300
    orchestrator._rate_limit_resume_default_seconds = 900
    orchestrator._exec_mode = "python"
    return orchestrator


def test_handle_retry_timer_dispatches_due_issue() -> None:
    orchestrator = _orchestrator_stub()
    issue = _issue()
    retry_entry = RetryEntry(
        issue_id=issue.id,
        identifier=issue.identifier,
        attempt=2,
        due_at_monotonic=0.0,
        error="hook_failed",
    )
    dispatched: list[tuple[str, int | None]] = []
    scheduled: list[tuple[str, str, int, str | None, bool]] = []

    orchestrator._has_available_global_slot = lambda: True
    orchestrator._is_dispatch_eligible = lambda candidate, from_retry: True
    orchestrator._release_claim = lambda issue_id: None
    orchestrator._dispatch_issue = (
        lambda candidate, attempt: dispatched.append(  # type: ignore[assignment]
            (candidate.id, attempt)
        )
        or True
    )
    orchestrator._schedule_retry = (  # type: ignore[assignment]
        lambda issue_id, identifier, attempt, *, error, continuation: scheduled.append(
            (issue_id, identifier, attempt, error, continuation)
        )
    )

    orchestrator._handle_retry_timer(retry_entry, {issue.id: issue})

    assert dispatched == [(issue.id, retry_entry.attempt)]
    assert scheduled == []


def test_snapshot_state_uses_identifier_index_for_attention() -> None:
    orchestrator = _orchestrator_stub()
    issue_id = "d0bc450d-8b47-47b5-ad51-839222bf5d9a"
    with orchestrator._state_lock:
        orchestrator._state.last_errors[issue_id] = "hook_failed"
        orchestrator._state.issue_identifiers[issue_id] = "MOL-7"

    snapshot = orchestrator.snapshot_state()
    attention = snapshot["attention"]
    assert attention
    assert attention[0]["issue_identifier"] == "MOL-7"


def test_snapshot_issue_returns_blocked_for_orphaned_issue() -> None:
    orchestrator = _orchestrator_stub()
    issue_id = "d0bc450d-8b47-47b5-ad51-839222bf5d9a"
    with orchestrator._state_lock:
        orchestrator._state.claimed.add(issue_id)
        orchestrator._state.last_errors[issue_id] = "hook_failed"
        orchestrator._state.issue_identifiers[issue_id] = "MOL-7"

    payload = orchestrator.snapshot_issue("MOL-7")
    assert payload is not None
    assert payload["status"] == "blocked"
    assert payload["issue_id"] == issue_id
    assert payload["last_error"] == "hook_failed"


def test_request_retry_now_recovers_orphaned_claim() -> None:
    orchestrator = _orchestrator_stub()
    issue_id = "d0bc450d-8b47-47b5-ad51-839222bf5d9a"
    scheduled: list[tuple[str, str, int, str | None, bool]] = []
    orchestrator._schedule_retry = (  # type: ignore[assignment]
        lambda issue_id, identifier, attempt, *, error, continuation: scheduled.append(
            (issue_id, identifier, attempt, error, continuation)
        )
    )

    with orchestrator._state_lock:
        orchestrator._state.claimed.add(issue_id)
        orchestrator._state.last_errors[issue_id] = "hook_failed"
        orchestrator._state.issue_identifiers[issue_id] = "MOL-7"

    payload = orchestrator.request_retry_now("MOL-7")

    assert payload["ok"] is True
    assert payload["status"] == "retrying"
    assert scheduled == [
        (issue_id, "MOL-7", 1, "manual_retry_recovered", True),
    ]


def test_snapshot_state_surfaces_system_suspension_attention() -> None:
    orchestrator = _orchestrator_stub()
    with orchestrator._state_lock:
        orchestrator._set_suspension_locked(
            kind="auth_required",
            message="Please run codex login.",
            resume_delay_seconds=300,
            auto_resume=True,
        )
    snapshot = orchestrator.snapshot_state()
    assert snapshot["suspension"]["active"] is True
    attention = snapshot["attention"]
    assert attention
    assert attention[0]["issue_identifier"] == "SYSTEM"
    assert attention[0]["kind"] == "auth_required"


def test_on_worker_exit_turn_input_required_sets_auth_pause() -> None:
    orchestrator = _orchestrator_stub()
    issue = _issue("MOL-19", "issue-19")
    running_entry = RunningEntry(
        issue=issue,
        issue_identifier=issue.identifier,
        worker_name="symphony-executor-MOL-19",
        worker_role="executor",
        started_at_utc=now_utc(),
        started_at_monotonic=0.0,
        retry_attempt=1,
    )
    with orchestrator._state_lock:
        orchestrator._state.running[issue.id] = running_entry
        orchestrator._state.claimed.add(issue.id)
    scheduled: list[tuple[str, str, int, str | None, bool, int | None]] = []
    orchestrator._schedule_retry = (  # type: ignore[assignment]
        lambda issue_id,
        identifier,
        attempt,
        *,
        error,
        continuation,
        delay_override_ms=None: scheduled.append(
            (
                issue_id,
                identifier,
                attempt,
                error,
                continuation,
                delay_override_ms,
            )
        )
    )

    orchestrator._on_worker_exit(
        {
            "issue_id": issue.id,
            "reason": "turn_input_required",
            "error": "turn_input_required",
            "duration_seconds": 1.0,
            "final_issue": issue,
        }
    )

    with orchestrator._state_lock:
        assert orchestrator._state.suspension_kind == "auth_required"
        assert orchestrator._state.suspension_auto_resume is True
    assert scheduled
    assert scheduled[0][0] == issue.id
    assert scheduled[0][3] == "auth_required"
    assert scheduled[0][5] == orchestrator._auth_resume_delay_seconds * 1000


def test_rate_limit_suspension_derivation() -> None:
    payload = {
        "primary": {
            "usedPercent": 100.0,
            "windowDuration": 3600,
        }
    }
    suspension = _derive_rate_limit_suspension(
        payload,
        default_resume_seconds=900,
    )
    assert suspension is not None
    assert suspension["resume_seconds"] == 3600
