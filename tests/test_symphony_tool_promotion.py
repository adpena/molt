from __future__ import annotations

from pathlib import Path

from molt.symphony.tool_promotion import ToolPromotionStore


def test_tool_promotion_distills_recurring_successful_actions(tmp_path: Path) -> None:
    store = ToolPromotionStore(
        events_path=tmp_path / "tool_promotion.jsonl",
        distillations_dir=tmp_path / "distillations",
    )
    payload = store.distill_candidates(
        taste_rows=[
            {
                "recorded_at": "2026-03-05T00:00:00Z",
                "successful_actions": [
                    "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_dlq.py summary --limit 20"
                ],
                "tools_used": ["readiness_audit", "linear_hygiene"],
            },
            {
                "recorded_at": "2026-03-05T00:01:00Z",
                "successful_actions": [
                    "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_dlq.py summary --limit 20"
                ],
                "tools_used": ["readiness_audit"],
            },
            {
                "recorded_at": "2026-03-05T00:02:00Z",
                "successful_actions": [
                    "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_dlq.py summary --limit 20"
                ],
                "tools_used": ["readiness_audit"],
            },
        ],
        min_success_count=3,
    )
    assert payload["candidate_count"] == 1
    assert payload["ready_candidate_count"] == 1
    candidate = payload["ready_candidates"][0]
    assert candidate["ready"] is True
    assert candidate["success_count"] == 3
    assert Path(str(payload["path"])).exists()
    assert payload["manifest_batch"]["manifest_count"] == 1
    manifest_path = Path(str(payload["manifest_batch"]["manifests"][0]["path"]))
    assert manifest_path.exists()


def test_tool_promotion_record_and_load_round_trip(tmp_path: Path) -> None:
    store = ToolPromotionStore(
        events_path=tmp_path / "tool_promotion.jsonl",
        distillations_dir=tmp_path / "distillations",
    )
    row = store.record(
        {
            "kind": "tool_promotion_distillation",
            "candidate_count": 2,
            "ready_candidate_count": 1,
            "path": str(tmp_path / "distillations" / "tool_promotion.json"),
        }
    )
    loaded = store.load(limit=10)
    assert len(loaded) == 1
    assert loaded[0]["kind"] == "tool_promotion_distillation"
    assert loaded[0]["candidate_count"] == 2
    assert "recorded_at" in row
