from __future__ import annotations

import json
from types import SimpleNamespace

import pytest

import tools.linear_hygiene as linear_hygiene


def test_sanitize_issue_title_removes_trailing_noise() -> None:
    assert (
        linear_hygiene.sanitize_issue_title(
            "[P1][RT2] async I/O cancellation propagation.) |"
        )
        == "[P1][RT2] async I/O cancellation propagation)"
    )
    assert (
        linear_hygiene.sanitize_issue_title(
            "[P1][DB2] wasm connector parity with cancellation bytes))"
        )
        == "[P1][DB2] wasm connector parity with cancellation bytes)"
    )


def test_canonicalize_title_normalizes_variants() -> None:
    left = linear_hygiene.canonicalize_title(
        "[P1][RT2] async I/O cancellation propagation.)"
    )
    right = linear_hygiene.canonicalize_title(
        "[P1][RT2] async I/O cancellation propagation)"
    )
    assert left == right


def test_infer_seed_metadata_fills_missing_source_from_manifest() -> None:
    issue = {
        "title": "[P1][RT2] async I/O cancellation propagation.)",
        "description": (
            "Auto-seeded from Molt roadmap/status TODO contracts.\n\n"
            "Original TODO: async I/O cancellation propagation\n"
            "Area: async-runtime\n"
            "Owner lane: runtime\n"
            "Milestone: RT2\n"
            "Status tag: partial\n"
        ),
    }
    lookup = {
        linear_hygiene.canonicalize_title(
            "[P1][RT2] async I/O cancellation propagation)"
        ): {
            "source": "ROADMAP.md:123",
            "priority": "P1",
            "area": "async-runtime",
            "owner": "runtime",
            "milestone": "RT2",
            "status": "partial",
        }
    }
    metadata = linear_hygiene._infer_seed_metadata(issue=issue, manifest_lookup=lookup)
    assert metadata is not None
    assert metadata["source"] == "ROADMAP.md:123"
    assert metadata["priority"] == "P1"
    assert metadata["owner"] == "runtime"


def test_run_formal_suite_warns_on_runtime_mismatch(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    payload = {
        "ok": False,
        "checks": {
            "quint": {"diagnostics": {"runtime_mismatch_detected": True}, "errors": []}
        },
    }
    monkeypatch.setattr(
        linear_hygiene.subprocess,
        "run",
        lambda *args, **kwargs: SimpleNamespace(
            returncode=1,
            stdout=json.dumps(payload),
            stderr="",
        ),
    )
    result = linear_hygiene._run_formal_suite("quint")
    assert result["status"] == "warn"
    assert "--quint" in result["command"]


def test_run_formal_suite_rejects_invalid_mode() -> None:
    with pytest.raises(RuntimeError):
        linear_hygiene._run_formal_suite("invalid")
