"""Teeth for the A9 warn->strict flip auditor."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import tools.check_gate_flips as cgf  # noqa: E402


def test_strict_entry_is_ok():
    v = cgf.evaluate_flip(
        {"name": "g", "state": "strict", "strict_when": "landed_functional"}
    )
    assert v.status == "ok-strict"


def test_warn_with_met_condition_should_flip():
    v = cgf.evaluate_flip(
        {"name": "g", "state": "warn", "strict_when": "live_count == 0"}, live_count=0
    )
    assert v.status == "should-flip"


def test_warn_with_unmet_condition_still_warn():
    v = cgf.evaluate_flip(
        {"name": "g", "state": "warn", "strict_when": "live_count == 0"}, live_count=3
    )
    assert v.status == "still-warn"


def test_warn_with_le_condition():
    assert (
        cgf.evaluate_flip(
            {"name": "g", "state": "warn", "strict_when": "live_count <= 5"},
            live_count=5,
        ).status
        == "should-flip"
    )


def test_free_text_condition_is_manual():
    v = cgf.evaluate_flip(
        {"name": "g", "state": "warn", "strict_when": "when the operator says so"}
    )
    assert v.status == "manual"


def test_landed_functional_warn_is_manual_not_flip():
    # A warn gate whose marker is a lifecycle token, not a trigger.
    v = cgf.evaluate_flip(
        {"name": "g", "state": "warn", "strict_when": "landed_functional"}
    )
    assert v.status == "manual"


def _write_config(tmp_path, body):
    p = tmp_path / "gates.toml"
    p.write_text(body, encoding="utf-8")
    return p


def test_main_check_fails_on_met_but_unflipped(tmp_path, monkeypatch):
    detector = tmp_path / "count_zero.py"
    detector.write_text("print(0)\n", encoding="utf-8")
    monkeypatch.setitem(
        cgf.COUNT_DETECTORS,
        "test_zero",
        cgf.CountDetector(str(detector), ()),
    )
    body = (
        "[[gate_flip]]\n"
        'name = "leaky"\n'
        'state = "warn"\n'
        'strict_when = "live_count == 0"\n'
        'count_detector = "test_zero"\n'
    )
    cfg = _write_config(tmp_path, body)
    assert cgf.main(["--config", str(cfg), "--check"]) == 1


def test_legacy_shell_count_command_is_rejected(tmp_path):
    cfg = _write_config(
        tmp_path,
        "[[gate_flip]]\n"
        'name = "legacy"\n'
        'state = "warn"\n'
        'strict_when = "live_count == 0"\n'
        'count_cmd = "python -c \\"print(0)\\""\n',
    )

    assert cgf.main(["--config", str(cfg), "--check"]) == 2


def test_json_detector_extracts_typed_non_negative_count():
    detector = cgf.CountDetector("ignored.py", (), ("counts", "stale"))

    assert detector.parse('{"counts": {"stale": 7}}') == 7
    with pytest.raises(ValueError, match="must be an integer"):
        detector.parse('{"counts": {"stale": 7.5}}')


def test_main_check_passes_when_all_strict(tmp_path):
    body = (
        "[[gate_flip]]\n"
        'name = "done"\n'
        'state = "strict"\n'
        'strict_when = "landed_functional"\n'
    )
    cfg = _write_config(tmp_path, body)
    assert cgf.main(["--config", str(cfg), "--check"]) == 0


def test_committed_registry_is_clean():
    # The shipped proof_plan.toml gate_flip registry must not be in drift.
    assert cgf.main(["--config", str(cgf.DEFAULT_CONFIG), "--check"]) == 0
