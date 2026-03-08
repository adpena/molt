from __future__ import annotations

from pathlib import Path

from molt.symphony.taste_memory import TasteMemoryStore


def test_taste_memory_distills_recent_patterns(tmp_path: Path) -> None:
    store = TasteMemoryStore(
        events_path=tmp_path / "events.jsonl",
        distillations_dir=tmp_path / "distillations",
    )
    store.record(
        {
            "cycle_status": "fail",
            "failure_codes": ["formal_pass_ratio_low"],
            "successful_actions": [],
            "tools_used": ["readiness_audit", "linear_hygiene"],
        }
    )
    store.record(
        {
            "cycle_status": "pass",
            "failure_codes": [],
            "successful_actions": [
                "PYTHONPATH=src uv run --python 3.12 python3 tools/check_formal_methods.py --json-only"
            ],
            "tools_used": ["readiness_audit", "linear_hygiene"],
        }
    )
    out = store.distill_recent(limit=20)
    assert out["samples"] == 2
    assert out["recurring_failure_codes"]["formal_pass_ratio_low"] == 1
    assert out["preferred_tools"]["readiness_audit"] == 2
    assert Path(out["path"]).exists()
