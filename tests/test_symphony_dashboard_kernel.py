from __future__ import annotations

from pathlib import Path

from molt.symphony.dashboard_kernel import (
    classify_event_tone,
    classify_trace_status,
    compact_recent_events,
)
from tools.symphony_dashboard_wasm import build_dashboard_wasm_cmd


def test_classify_event_tone_matrix() -> None:
    assert classify_event_tone("task_complete") == "ok"
    assert classify_event_tone("turn_failed") == "danger"
    assert classify_event_tone("thread/tokenUsage/updated") == "info"
    assert classify_event_tone("retry_due") == "warn"


def test_classify_trace_status_matrix() -> None:
    assert classify_trace_status("running") == "status-running"
    assert classify_trace_status("retrying") == "status-retrying"
    assert classify_trace_status("blocked") == "status-blocked"
    assert classify_trace_status("unknown") == ""


def test_compact_recent_events_shapes_rows() -> None:
    rows = compact_recent_events(
        [
            {"event": "task_complete", "message": "done", "at": "2026-03-05T00:00:00Z"},
            {"event": "token_count", "message": "123"},
        ],
        limit=1,
    )
    assert len(rows) == 1
    assert rows[0]["event"] == "task_complete"
    assert rows[0]["tone"] == "ok"


def test_build_dashboard_wasm_cmd_includes_linked_output() -> None:
    cmd = build_dashboard_wasm_cmd(
        source=Path("/repo/src/molt/symphony/dashboard_kernel.py"),
        output=Path("/Volumes/APDataStore/Molt/wasm/symphony/dashboard_kernel.wasm"),
        profile="release",
        linked=True,
    )
    assert cmd[:7] == ["uv", "run", "--python", "3.12", "python", "-m", "molt.cli"]
    assert "--target" in cmd
    assert "wasm" in cmd
    assert "--linked" in cmd
    assert "--linked-output" in cmd
    assert "--profile" in cmd
    assert "release" in cmd
