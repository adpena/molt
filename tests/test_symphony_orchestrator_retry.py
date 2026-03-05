from __future__ import annotations

import queue
import threading
from datetime import UTC, datetime
from pathlib import Path
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
    _quint_command_with_fallback,
    _derive_rate_limit_suspension,
    _extract_retry_schedule_from_error,
    _extract_retry_delay_seconds_from_error,
    _resolve_formal_spec_path,
    _resolve_quint_workdir,
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
    orchestrator._event_queue = queue.Queue()
    orchestrator._event_queue_max = 8192
    orchestrator._event_queue_drop_log_interval = 250
    orchestrator._config = SimpleNamespace(
        codex=SimpleNamespace(command="codex"),
        agent=SimpleNamespace(
            default_role="executor",
            role_pools={},
            max_concurrent_agents=4,
            max_concurrent_agents_by_state={"todo": 4, "in progress": 4, "rework": 2},
        ),
    )
    orchestrator._worker_controls = {}
    orchestrator._auth_resume_delay_seconds = 300
    orchestrator._rate_limit_resume_default_seconds = 900
    orchestrator._max_codex_event_counters = 8
    orchestrator._tool_state_default_detail = "compact"
    orchestrator._tool_state_running_limit = 8
    orchestrator._tool_state_retry_limit = 8
    orchestrator._tool_state_attention_limit = 8
    orchestrator._tool_state_events_limit = 12
    orchestrator._tool_state_cache_ttl_seconds = 60.0
    orchestrator._tool_state_cache = {}
    orchestrator._profiling_checkpoint_interval_seconds = 20.0
    orchestrator._profiling_compare_recent_window = 48
    orchestrator._profiling_baseline_max_events = 2400
    orchestrator._profiling_baseline_max_labels = 64
    orchestrator._last_profiling_checkpoint_monotonic = 0.0
    orchestrator._durable_memory = None
    orchestrator._exec_mode = "python"
    orchestrator._server_port = 8089
    orchestrator._perf_reports_dir = Path("/tmp")
    orchestrator._perf_verdict_path = Path("/tmp/molt_symphony_perf_verdict.json")
    orchestrator._perf_verdict_cache_ttl_seconds = 2.0
    orchestrator._perf_verdict_cache = None
    orchestrator._perf_guard_timeout_seconds = 30
    orchestrator._perf_guard_running = False
    orchestrator._perf_guard_last_started_at = None
    orchestrator._perf_guard_last_finished_at = None
    orchestrator._perf_guard_last_result = None
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
    orchestrator._dispatch_issue = lambda candidate, attempt: (
        dispatched.append(  # type: ignore[assignment]
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


def test_snapshot_state_retry_rows_handle_orphaned_claims_without_crashing() -> None:
    orchestrator = _orchestrator_stub()
    issue = _issue()
    with orchestrator._state_lock:
        orchestrator._state.claimed.add(issue.id)
        orchestrator._state.retry_attempts[issue.id] = RetryEntry(
            issue_id=issue.id,
            identifier=issue.identifier,
            attempt=3,
            due_at_monotonic=0.0,
            error="hook_failed",
        )

    snapshot = orchestrator.snapshot_state()
    assert snapshot["retrying"]
    retry_row = snapshot["retrying"][0]
    assert retry_row["issue_identifier"] == issue.identifier
    assert retry_row["title"] is None


def test_snapshot_state_includes_event_queue_runtime_telemetry() -> None:
    orchestrator = _orchestrator_stub()
    orchestrator._event_queue.put(("codex_update", {"issue_id": "x"}))
    with orchestrator._state_lock:
        orchestrator._state.profiling.incr("events_dropped", 3)
    snapshot = orchestrator.snapshot_state()
    queue_payload = snapshot["runtime"]["event_queue"]
    assert queue_payload["depth"] == 1
    assert queue_payload["max"] == orchestrator._event_queue_max
    assert queue_payload["dropped_events"] == 3
    assert queue_payload["utilization"] > 0
    profiling_compare = snapshot["profiling_compare"]
    assert profiling_compare["baseline_checkpoint_samples"] == 0
    assert isinstance(profiling_compare["optimizations"], list)


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


def test_snapshot_issue_accepts_issue_id_lookup_for_orphaned_issue() -> None:
    orchestrator = _orchestrator_stub()
    issue_id = "d0bc450d-8b47-47b5-ad51-839222bf5d9a"
    with orchestrator._state_lock:
        orchestrator._state.claimed.add(issue_id)
        orchestrator._state.last_errors[issue_id] = "hook_failed"
        orchestrator._state.issue_identifiers[issue_id] = "MOL-7"

    payload = orchestrator.snapshot_issue(issue_id)
    assert payload is not None
    assert payload["status"] == "blocked"
    assert payload["issue_id"] == issue_id
    assert payload["issue_identifier"] == "MOL-7"


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


def test_request_retry_now_accepts_issue_id_lookup() -> None:
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

    payload = orchestrator.request_retry_now(issue_id)

    assert payload["ok"] is True
    assert payload["status"] == "retrying"
    assert payload["issue_identifier"] == "MOL-7"
    assert scheduled == [
        (issue_id, "MOL-7", 1, "manual_retry_recovered", True),
    ]


def test_request_stop_now_accepts_issue_id_lookup() -> None:
    orchestrator = _orchestrator_stub()
    issue = _issue("MOL-21", "issue-21")
    running_entry = RunningEntry(
        issue=issue,
        issue_identifier=issue.identifier,
        worker_name="symphony-executor-MOL-21",
        worker_role="executor",
        started_at_utc=now_utc(),
        started_at_monotonic=0.0,
        retry_attempt=1,
    )
    with orchestrator._state_lock:
        orchestrator._state.running[issue.id] = running_entry
        orchestrator._state.claimed.add(issue.id)
        orchestrator._state.issue_identifiers[issue.id] = issue.identifier

    payload = orchestrator.request_stop_now(issue.id)
    assert payload["ok"] is True
    assert payload["issue_identifier"] == issue.identifier
    with orchestrator._state_lock:
        assert orchestrator._state.running[issue.id].stop_requested is True
        assert orchestrator._state.running[issue.id].cleanup_reason == "manual_stop"


def test_on_worker_exit_manual_stop_pauses_until_manual_retry() -> None:
    orchestrator = _orchestrator_stub()
    issue = _issue("MOL-22", "issue-22")
    running_entry = RunningEntry(
        issue=issue,
        issue_identifier=issue.identifier,
        worker_name="symphony-executor-MOL-22",
        worker_role="executor",
        started_at_utc=now_utc(),
        started_at_monotonic=0.0,
        retry_attempt=2,
        stop_requested=True,
        cleanup_reason="manual_stop",
    )
    with orchestrator._state_lock:
        orchestrator._state.running[issue.id] = running_entry
        orchestrator._state.claimed.add(issue.id)
        orchestrator._state.issue_identifiers[issue.id] = issue.identifier

    orchestrator._on_worker_exit(
        {
            "issue_id": issue.id,
            "reason": "cancelled",
            "error": "manual_stop",
            "duration_seconds": 0.25,
        }
    )

    with orchestrator._state_lock:
        assert issue.id not in orchestrator._state.retry_attempts
        assert orchestrator._state.last_errors[issue.id] == "paused_by_operator"
        assert issue.id in orchestrator._state.claimed


def test_set_max_concurrent_agents_tool_updates_limits() -> None:
    orchestrator = _orchestrator_stub()
    result = orchestrator.run_dashboard_tool(
        "set_max_concurrent_agents",
        {"value": "2"},
    )
    assert result["ok"] is True
    assert result["status"] == "updated"
    assert result["max_concurrent_agents"] == 2
    with orchestrator._state_lock:
        assert orchestrator._state.max_concurrent_agents == 2
    assert orchestrator._config.agent.max_concurrent_agents == 2
    assert orchestrator._config.agent.max_concurrent_agents_by_state["todo"] == 2
    assert orchestrator._config.agent.max_concurrent_agents_by_state["in progress"] == 2
    assert orchestrator._config.agent.max_concurrent_agents_by_state["rework"] == 2


def test_set_max_concurrent_agents_tool_rejects_invalid_value() -> None:
    orchestrator = _orchestrator_stub()
    result = orchestrator.run_dashboard_tool(
        "set_max_concurrent_agents",
        {"value": "nope"},
    )
    assert result["ok"] is False
    assert result["error"] == "invalid_value"


def test_quint_command_with_fallback_uses_prefix_when_available(
    monkeypatch,
) -> None:
    monkeypatch.setenv("MOLT_QUINT_NODE_FALLBACK", "npx -y node@22")
    monkeypatch.setattr(
        "molt.symphony.orchestrator.shutil.which",
        lambda name: "/usr/bin/npx" if name == "npx" else "/usr/bin/quint",
    )
    cmd = _quint_command_with_fallback(["run", "formal/quint/example.qnt"])
    assert cmd[:4] == ["npx", "-y", "node@22", "/usr/bin/quint"]
    assert cmd[-2:] == ["run", "formal/quint/example.qnt"]


def test_quint_command_with_fallback_skips_missing_prefix_launcher(
    monkeypatch,
) -> None:
    monkeypatch.setenv(
        "MOLT_QUINT_NODE_FALLBACK", "/definitely/missing/launcher --flag"
    )

    def _which(name: str) -> str | None:
        if name == "quint":
            return "/usr/bin/quint"
        return None

    monkeypatch.setattr("molt.symphony.orchestrator.shutil.which", _which)
    cmd = _quint_command_with_fallback(["verify", "formal/quint/example.qnt"])
    assert cmd == ["quint", "verify", "formal/quint/example.qnt"]


def test_resolve_formal_spec_path_returns_absolute(tmp_path) -> None:
    workspace = tmp_path / "ws"
    workspace.mkdir()
    resolved = _resolve_formal_spec_path(workspace, "formal/quint/example.qnt")
    assert resolved == str((workspace / "formal/quint/example.qnt").resolve())


def test_resolve_quint_workdir_prefers_env(monkeypatch, tmp_path) -> None:
    workdir = tmp_path / "apalache"
    monkeypatch.setenv("MOLT_APALACHE_WORK_DIR", str(workdir))
    resolved = _resolve_quint_workdir()
    assert resolved == workdir.resolve()
    assert resolved.exists()


def test_snapshot_state_surfaces_system_suspension_attention() -> None:
    orchestrator = _orchestrator_stub()
    with orchestrator._state_lock:
        orchestrator._set_suspension_locked(
            kind="auth_required",
            message="Please run codex login.",
            resume_delay_seconds=300,
            auto_resume=True,
            resume_source="auth",
            resume_reason="auth_required",
        )
    snapshot = orchestrator.snapshot_state()
    assert snapshot["suspension"]["active"] is True
    assert snapshot["suspension"]["resume_source"] == "auth"
    assert snapshot["suspension"]["resume_reason"] == "auth_required"
    assert snapshot["suspension"]["resume_at_epoch_seconds"] is not None
    assert snapshot["suspension"]["resume_at"] is not None
    attention = snapshot["attention"]
    assert attention
    assert attention[0]["issue_identifier"] == "SYSTEM"
    assert attention[0]["kind"] == "auth_required"


def test_snapshot_state_surfaces_perf_guard_breach_attention(tmp_path) -> None:
    orchestrator = _orchestrator_stub()
    verdict_path = tmp_path / "verdict.json"
    verdict_path.write_text(
        '{"status":"breach","breaches_count":2,"report_path":"x"}',
        encoding="utf-8",
    )
    orchestrator._perf_verdict_path = verdict_path
    orchestrator._perf_verdict_cache = None
    snapshot = orchestrator.snapshot_state()
    attention = snapshot["attention"]
    assert attention
    assert any(
        str(row.get("kind") or "").startswith("perf_guard_")
        for row in attention
        if isinstance(row, dict)
    )
    perf_guard = snapshot["runtime"]["perf_guard"]
    assert perf_guard["verdict"]["status"] == "breach"


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
        lambda issue_id, identifier, attempt, *, error, continuation, delay_override_ms=None: (
            scheduled.append(
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


def test_rate_limit_suspension_credit_pause_prefers_reset_epoch() -> None:
    payload = {
        "credits": {
            "hasCredits": False,
            "resetsAt": 1125,
        }
    }
    suspension = _derive_rate_limit_suspension(
        payload,
        default_resume_seconds=900,
        now_epoch_seconds=1000,
    )
    assert suspension is not None
    assert suspension["reason"] == "credits_exhausted"
    assert suspension["resume_seconds"] == 125
    assert suspension["resume_at_epoch_seconds"] == 1125


def test_worker_exit_rate_limited_sets_suspension_and_retry_delay() -> None:
    orchestrator = _orchestrator_stub()
    issue = _issue("MOL-11", "issue-11")
    running_entry = RunningEntry(
        issue=issue,
        issue_identifier=issue.identifier,
        worker_name="symphony-executor-MOL-11",
        worker_role="executor",
        started_at_utc=now_utc(),
        started_at_monotonic=0.0,
        retry_attempt=1,
    )
    with orchestrator._state_lock:
        orchestrator._state.running[issue.id] = running_entry
        orchestrator._state.claimed.add(issue.id)
        orchestrator._state.codex_rate_limits = {
            "primary": {"usedPercent": 100.0, "windowDuration": 60}
        }
    scheduled: list[tuple[str, str, int, str | None, bool, int | None]] = []
    orchestrator._schedule_retry = (  # type: ignore[assignment]
        lambda issue_id, identifier, attempt, *, error, continuation, delay_override_ms=None: (
            scheduled.append(
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
    )

    orchestrator._on_worker_exit(
        {
            "issue_id": issue.id,
            "reason": "failed",
            "error": "rate limit exhausted, retry after 45s",
            "duration_seconds": 1.0,
            "final_issue": issue,
        }
    )

    with orchestrator._state_lock:
        assert orchestrator._state.suspension_kind == "rate_limited"
        assert orchestrator._state.suspension_auto_resume is True
    assert scheduled
    assert scheduled[0][0] == issue.id
    assert scheduled[0][5] == 60_000


def test_rate_limit_suspension_uses_reset_epoch_precisely() -> None:
    payload = {
        "primary": {
            "usedPercent": 100.0,
            "resetsAt": 1123,
        }
    }
    suspension = _derive_rate_limit_suspension(
        payload,
        default_resume_seconds=900,
        now_epoch_seconds=1000,
    )
    assert suspension is not None
    assert suspension["resume_seconds"] == 123


def test_rate_limit_suspension_uses_retry_after_ms() -> None:
    payload = {
        "primary": {
            "remaining": 0,
            "retry_after_ms": 4500,
        }
    }
    suspension = _derive_rate_limit_suspension(
        payload,
        default_resume_seconds=900,
        now_epoch_seconds=1000,
    )
    assert suspension is not None
    assert suspension["resume_seconds"] == 5


def test_extract_retry_delay_seconds_from_error_hint() -> None:
    seconds = _extract_retry_delay_seconds_from_error(
        "HTTP 429 rate limit hit, retry after 75s",
        default_resume_seconds=900,
    )
    assert seconds == 75


def test_extract_retry_delay_seconds_from_error_http_date(
    monkeypatch,
) -> None:
    monkeypatch.setattr("molt.symphony.orchestrator.time.time", lambda: 1000.0)
    seconds = _extract_retry_delay_seconds_from_error(
        "429 Too Many Requests; Retry-After: Thu, 01 Jan 1970 00:20:00 GMT",
        default_resume_seconds=900,
    )
    assert seconds == 200


def test_extract_retry_schedule_from_payload_reset_epoch() -> None:
    payload = {"primary": {"remaining": 0, "resetsAt": 1120}}
    schedule = _extract_retry_schedule_from_error(
        payload,
        default_resume_seconds=900,
    )
    assert schedule is not None
    assert schedule["resume_seconds"] >= 1
    assert schedule["source"] == "error.payload"


def test_codex_event_counter_cardinality_caps_to_other() -> None:
    orchestrator = _orchestrator_stub()
    orchestrator._max_codex_event_counters = 2
    with orchestrator._state_lock:
        orchestrator._record_codex_event_counter_locked("session_started")
        orchestrator._record_codex_event_counter_locked("turn_completed")
        orchestrator._record_codex_event_counter_locked("some_new_event")
    counters = orchestrator._state.profiling.counters
    assert counters["codex_event_session_started"] == 1
    assert counters["codex_event_turn_completed"] == 1
    assert counters["codex_event_other"] == 1


def test_tool_symphony_state_returns_compact_payload_by_default() -> None:
    orchestrator = _orchestrator_stub()
    issue = _issue("MOL-33", "issue-33")
    running_entry = RunningEntry(
        issue=issue,
        issue_identifier=issue.identifier,
        worker_name="symphony-executor-MOL-33",
        worker_role="executor",
        started_at_utc=now_utc(),
        started_at_monotonic=0.0,
        retry_attempt=1,
    )
    with orchestrator._state_lock:
        orchestrator._state.running[issue.id] = running_entry
        orchestrator._state.last_errors[issue.id] = "hook_failed"
        orchestrator._state.issue_identifiers[issue.id] = issue.identifier
    payload = orchestrator._tool_symphony_state(None)
    assert payload["success"] is True
    state = payload["state"]
    assert state["compact"] is True
    assert "agent_panes" not in state
    assert state["running"]


def test_tool_symphony_state_supports_agent_telemetry_detail() -> None:
    orchestrator = _orchestrator_stub()
    payload = orchestrator._tool_symphony_state({"detail": "telemetry"})
    assert payload["success"] is True
    state = payload["state"]
    assert state["telemetry"] is True
    assert state["compact"] is True
    assert "throughput" in state


def test_tool_symphony_state_compact_cache_hit() -> None:
    orchestrator = _orchestrator_stub()
    first = orchestrator._tool_symphony_state({"detail": "compact"})
    second = orchestrator._tool_symphony_state({"detail": "compact"})
    assert first["success"] is True
    assert second["success"] is True
    assert second.get("cached") is True


def test_snapshot_durable_memory_disabled_payload() -> None:
    orchestrator = _orchestrator_stub()
    payload = orchestrator.snapshot_durable_memory(limit=42)
    assert payload["enabled"] is False
    assert payload["reason"] == "durable_memory_disabled"


def test_snapshot_durable_memory_enabled_payload() -> None:
    orchestrator = _orchestrator_stub()

    class _Store:
        def summary(self, *, limit: int = 120) -> dict[str, object]:
            return {
                "enabled": True,
                "root": "/Volumes/APDataStore/Molt/logs/symphony/durable_memory",
                "recent_events": [{"kind": "codex_event"}][: max(limit, 1)],
            }

    orchestrator._durable_memory = _Store()  # type: ignore[assignment]
    payload = orchestrator.snapshot_durable_memory(limit=5)
    assert payload["enabled"] is True
    assert payload["root"].endswith("durable_memory")
    assert payload["recent_events"]


def test_run_dashboard_tool_durable_backup_disabled_returns_error() -> None:
    orchestrator = _orchestrator_stub()
    payload = orchestrator.run_dashboard_tool("durable_backup", {})
    assert payload["ok"] is False
    assert payload["error"] == "durable_memory_disabled"


def test_run_dashboard_tool_durable_integrity_check_success() -> None:
    orchestrator = _orchestrator_stub()

    class _Store:
        def run_integrity_check(self) -> dict[str, object]:
            return {"ok": True, "checks": {"jsonl_readable": {"ok": True}}}

        def record(self, row: dict[str, object]) -> None:
            _ = row

    orchestrator._durable_memory = _Store()  # type: ignore[assignment]
    payload = orchestrator.run_dashboard_tool("durable_integrity_check", {})
    assert payload["ok"] is True
    assert payload["tool"] == "durable_integrity_check"
    assert "checks" in payload


def test_run_dashboard_tool_run_perf_guard_starts_background_job(monkeypatch) -> None:
    orchestrator = _orchestrator_stub()

    def _fake_job() -> None:
        with orchestrator._state_lock:
            orchestrator._perf_guard_running = False
            orchestrator._perf_guard_last_finished_at = (
                now_utc().isoformat().replace("+00:00", "Z")
            )
            orchestrator._perf_guard_last_result = {
                "status": "pass",
                "message": "ok",
                "finished_at": orchestrator._perf_guard_last_finished_at,
            }

    monkeypatch.setattr(orchestrator, "_run_perf_guard_job", _fake_job)
    payload = orchestrator.run_dashboard_tool("run_perf_guard", {})
    assert payload["ok"] is True
    assert payload["status"] == "started"
