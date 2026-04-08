from __future__ import annotations

import json
from pathlib import Path

from molt.debug.diff import (
    build_diff_summary_payload,
    load_diff_summary,
    load_failure_queue,
)


def test_load_diff_summary_and_failure_queue(tmp_path: Path) -> None:
    summary_path = tmp_path / "summary.json"
    summary_path.write_text(
        json.dumps(
            {
                "run_id": "run-123",
                "jobs": 2,
                "config": {"build_profile": "dev"},
                "discovered": 3,
                "total": 3,
                "passed": 2,
                "failed": 1,
                "skipped": 0,
                "oom": 0,
                "failed_files": ["tests/differential/basic/example.py"],
            }
        ),
        encoding="utf-8",
    )
    failures_path = tmp_path / "failures.txt"
    failures_path.write_text(
        "tests/differential/basic/example.py\n# comment\n\n", encoding="utf-8"
    )
    assert load_diff_summary(summary_path)["run_id"] == "run-123"
    assert load_failure_queue(failures_path) == ["tests/differential/basic/example.py"]


def test_build_diff_summary_payload_preserves_counts_and_failures() -> None:
    payload = build_diff_summary_payload(
        {
            "run_id": "run-xyz",
            "jobs": 1,
            "config": {"build_profile": "release"},
            "discovered": 1,
            "total": 1,
            "passed": 0,
            "failed": 1,
            "skipped": 0,
            "oom": 0,
            "failed_files": ["tests/differential/basic/example.py"],
        },
        failures=["tests/differential/basic/example.py"],
    )
    assert payload == {
        "run_id": "run-xyz",
        "jobs": 1,
        "counts": {
            "discovered": 1,
            "total": 1,
            "passed": 0,
            "failed": 1,
            "skipped": 0,
            "oom": 0,
        },
        "config": {"build_profile": "release"},
        "failed_files": ["tests/differential/basic/example.py"],
        "failure_queue": ["tests/differential/basic/example.py"],
    }
