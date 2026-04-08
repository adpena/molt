from __future__ import annotations

import json
from pathlib import Path
from typing import Any


def load_diff_summary(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def load_failure_queue(path: Path) -> list[str]:
    if not path.exists():
        return []
    failures: list[str] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        text = line.strip()
        if not text or text.startswith("#"):
            continue
        failures.append(text.split()[0])
    return failures


def build_diff_summary_payload(
    summary: dict[str, Any], *, failures: list[str] | None = None
) -> dict[str, Any]:
    return {
        "run_id": summary.get("run_id"),
        "jobs": summary.get("jobs"),
        "counts": {
            "discovered": summary.get("discovered", 0),
            "total": summary.get("total", 0),
            "passed": summary.get("passed", 0),
            "failed": summary.get("failed", 0),
            "skipped": summary.get("skipped", 0),
            "oom": summary.get("oom", 0),
        },
        "config": dict(summary.get("config", {})),
        "failed_files": list(summary.get("failed_files", [])),
        "failure_queue": list(failures or []),
    }
