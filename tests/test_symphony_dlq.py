from __future__ import annotations

from pathlib import Path

from molt.symphony.dlq import DeadLetterQueue, dead_letter_fingerprint


def test_dead_letter_queue_append_and_summary(tmp_path: Path) -> None:
    queue = DeadLetterQueue(tmp_path / "dlq.jsonl")
    queue.append(
        {
            "kind": "recursive_loop_step_failure",
            "name": "next_tranche_01",
            "command": ["echo fail"],
            "fingerprint": dead_letter_fingerprint(
                kind="recursive_loop_step_failure",
                name="next_tranche_01",
                command=["echo fail"],
            ),
        }
    )
    summary = queue.summary(limit=10)
    assert summary["count"] == 1
    assert summary["by_kind"]["recursive_loop_step_failure"] == 1
