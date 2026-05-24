from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
PROFILE_HOTSPOTS = REPO_ROOT / "tools" / "profile_hotspots.py"


def _load_profile_hotspots():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_profile_hotspots",
        PROFILE_HOTSPOTS,
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_profile_hotspots_summarizes_slowest_events(tmp_path: Path) -> None:
    module = _load_profile_hotspots()
    log_path = tmp_path / "commands.jsonl"
    events = [
        {
            "event": "guarded_command_profile",
            "prefix": "MOLT_TEST_SUITE",
            "status": "pass",
            "returncode": 0,
            "elapsed_s": 4.5,
            "recorded_at": "2026-05-24T00:00:01Z",
            "command": ["uv", "run", "pytest", "tests/test_a.py"],
        },
        {
            "event": "guarded_command_profile",
            "prefix": "MOLT_TEST_SUITE",
            "status": "pass",
            "returncode": 0,
            "elapsed_s": 12.25,
            "recorded_at": "2026-05-24T00:00:02Z",
            "command": ["cargo", "build", "--workspace"],
        },
    ]
    log_path.write_text(
        "".join(json.dumps(event, sort_keys=True) + "\n" for event in events),
        encoding="utf-8",
    )

    loaded = module.load_events([log_path])
    summary = module.summarize_events(loaded, limit=1)

    assert summary["total_events"] == 2
    assert summary["filtered_events"] == 2
    assert summary["total_elapsed_s"] == 16.75
    assert summary["slowest_events"][0]["elapsed_s"] == 12.25
    assert summary["slowest_events"][0]["short_command"] == "cargo build --workspace"
    assert summary["slowest_commands"][0]["count"] == 1
    assert summary["status_counts"] == {"pass": 2}
    assert "cargo build --workspace" in module.format_summary(summary)


def test_profile_hotspots_filters_by_prefix_and_min_elapsed(tmp_path: Path) -> None:
    module = _load_profile_hotspots()
    log_path = tmp_path / "commands.jsonl"
    events = [
        {
            "event": "guarded_command_profile",
            "prefix": "MOLT_BENCH",
            "status": "pass",
            "returncode": 0,
            "elapsed_s": 1.0,
            "command": ["python3", "quick.py"],
        },
        {
            "event": "guarded_command_profile",
            "prefix": "MOLT_BENCH",
            "status": "pass",
            "returncode": 0,
            "elapsed_s": 9.0,
            "command": ["python3", "slow.py"],
        },
        {
            "event": "guarded_command_profile",
            "prefix": "MOLT_DIFF",
            "status": "failed",
            "returncode": 1,
            "elapsed_s": 20.0,
            "command": ["python3", "diff.py"],
        },
    ]
    log_path.write_text(
        "".join(json.dumps(event, sort_keys=True) + "\n" for event in events),
        encoding="utf-8",
    )

    summary = module.summarize_events(
        module.load_events([log_path]),
        limit=5,
        min_elapsed_s=5.0,
        prefix="MOLT_BENCH",
    )

    assert summary["total_events"] == 3
    assert summary["filtered_events"] == 1
    assert summary["slowest_events"][0]["short_command"] == "python3 slow.py"
