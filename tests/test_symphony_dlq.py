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


def test_dead_letter_queue_health_clears_after_successful_replay(
    tmp_path: Path,
) -> None:
    queue = DeadLetterQueue(tmp_path / "dlq.jsonl")
    fingerprint = dead_letter_fingerprint(
        kind="recursive_loop_step_failure",
        name="next_tranche_01",
        command=["echo fail"],
    )
    queue.append(
        {
            "kind": "recursive_loop_step_failure",
            "name": "next_tranche_01",
            "phase": "next_tranche",
            "command": ["echo fail"],
            "fingerprint": fingerprint,
        }
    )
    before = queue.health_summary(limit=20)
    assert before["open_failure_count"] == 1
    queue.append_replay_result(
        target_fingerprint=fingerprint,
        command=["echo fail"],
        returncode=0,
    )
    after = queue.health_summary(limit=20)
    assert after["open_failure_count"] == 0
    assert after["replay_success_count"] == 1


def test_dead_letter_queue_health_marks_recurring_open_failures(tmp_path: Path) -> None:
    queue = DeadLetterQueue(tmp_path / "dlq.jsonl")
    fingerprint = dead_letter_fingerprint(
        kind="recursive_loop_step_failure",
        name="recurring",
        command=["echo fail"],
    )
    queue.append(
        {
            "kind": "recursive_loop_step_failure",
            "name": "recurring",
            "phase": "core",
            "command": ["echo fail"],
            "fingerprint": fingerprint,
        }
    )
    queue.append(
        {
            "kind": "recursive_loop_step_failure",
            "name": "recurring",
            "phase": "core",
            "command": ["echo fail"],
            "fingerprint": fingerprint,
        }
    )
    health = queue.health_summary(limit=20)
    assert health["open_failure_count"] == 1
    assert health["recurring_open_fingerprints"][fingerprint] == 2


def test_dead_letter_queue_recommended_replay_target_prefers_recurring_failure(
    tmp_path: Path,
) -> None:
    queue = DeadLetterQueue(tmp_path / "dlq.jsonl")
    queue.append(
        {
            "kind": "recursive_loop_step_failure",
            "name": "single",
            "phase": "core",
            "command": ["echo one"],
            "fingerprint": "single-fp",
        }
    )
    queue.append(
        {
            "kind": "recursive_loop_step_failure",
            "name": "recurring",
            "phase": "core",
            "command": ["echo two"],
            "fingerprint": "repeat-fp",
        }
    )
    queue.append(
        {
            "kind": "recursive_loop_step_failure",
            "name": "recurring",
            "phase": "core",
            "command": ["echo two"],
            "fingerprint": "repeat-fp",
        }
    )
    target = queue.recommended_replay_target(limit=20)
    assert target is not None
    assert target["fingerprint"] == "repeat-fp"
    assert target["count"] == 2
