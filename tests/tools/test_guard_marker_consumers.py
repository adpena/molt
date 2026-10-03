"""Every active-marker reader consumes the same parsed terminal evidence."""

from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt import pytest_memory_guard_bootstrap as bootstrap
from tools import disk_guard, runtime_wasm_final_preflight as preflight
from tools.memory_guard_core import active_custody as custody
from tools.proof_queue_pkg import diagnostic_evidence as diagnostic


def _launch(tmp_path):
    state = tmp_path / "tmp" / "memory_guard"
    marker = state / "active" / f"guard-10-{'a' * 32}.json"
    command = ["cargo", "build"]
    payload = {
        "schema_version": 2,
        "pid": 10,
        "token": "a" * 32,
        "guard_process": {"pid": 10, "started_at_ns": 100},
        "child_process": {"pid": 20, "started_at_ns": 200, "pgid": 20},
        "child_launch_state": "recorded",
        "status": "child_running",
        "command": command,
        "created_at": "2026-10-03T00:00:00Z",
        "path": str(bootstrap.SOURCE_ROOT / "tools" / "memory_guard.py"),
    }
    custody.write_active_guard_marker(marker, payload)
    env = {
        "MOLT_MEMORY_GUARD_STATE_ROOT": str(state),
        "MOLT_MEMORY_GUARD_TOKEN": payload["token"],
        "MOLT_MEMORY_GUARD_MARKER": str(marker),
    }
    summary = {
        "command": command,
        "recorded_at": payload["created_at"],
        "repro": {"env": env},
    }
    return marker, env, summary


def _queue_state(tmp_path, summary):
    return diagnostic._summary_guard_marker(
        {"cwd": str(tmp_path)}, summary, guard_pid=10
    )[0]


def test_reconciled_marker_is_terminal_for_every_reader(tmp_path):
    marker, env, summary = _launch(tmp_path)
    assert bootstrap._active_guard_marker_valid(env, guard_pid=10)
    assert disk_guard._has_active_guard(tmp_path)
    assert _queue_state(tmp_path, summary) == "nonterminal"
    assert preflight._active_build_guards((marker.parent,), live_pids=frozenset({10}))

    custody.reconcile_active_guard_markers(
        marker.parent,
        lambda: {99: SimpleNamespace(pid=99, started_at_ns=999, pgid=None)},
        apply=True,
    )
    assert not bootstrap._active_guard_marker_valid(env, guard_pid=10)
    assert not disk_guard._has_active_guard(tmp_path)
    assert _queue_state(tmp_path, summary) == "terminal"
    assert (
        preflight._active_build_guards((marker.parent,), live_pids=frozenset({10}))
        == []
    )


@pytest.mark.parametrize(
    "status", ["spawn_failed", "guard_exception", "finalizer_cleanup"]
)
def test_failed_or_unfinished_guard_cannot_claim_terminal_closure(tmp_path, status):
    marker, env, summary = _launch(tmp_path)
    custody.update_active_guard_marker(
        marker, env["MOLT_MEMORY_GUARD_TOKEN"], status=status
    )
    assert not bootstrap._active_guard_marker_valid(env, guard_pid=10)
    assert disk_guard._has_active_guard(tmp_path)
    assert _queue_state(tmp_path, summary) == "nonterminal"
    assert preflight._active_build_guards((marker.parent,), live_pids=frozenset({10}))


def test_malformed_terminal_is_rejected_by_every_reader(tmp_path):
    marker, env, summary = _launch(tmp_path)
    payload = json.loads(marker.read_text(encoding="utf-8"))
    payload.update(status="completed", token="wrong")
    marker.write_text(json.dumps(payload), encoding="utf-8")
    assert not bootstrap._active_guard_marker_valid(env, guard_pid=10)
    assert disk_guard._has_active_guard(tmp_path)
    assert _queue_state(tmp_path, summary) == "nonterminal"
    assert preflight._active_build_guards((marker.parent,), live_pids=frozenset())


def test_preflight_unreadable_marker_directory_remains_a_conflict(
    tmp_path, monkeypatch
):
    marker, _, _ = _launch(tmp_path)
    original = Path.iterdir

    def unreadable(path):
        if path == marker.parent:
            raise PermissionError("unreadable custody")
        return original(path)

    monkeypatch.setattr(Path, "iterdir", unreadable)
    conflicts = preflight._active_build_guards((marker.parent,), live_pids=frozenset())
    assert len(conflicts) == 1
    assert "unreadable custody" in conflicts[0]["error"]
