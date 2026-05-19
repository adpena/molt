from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
STREAM_TOOL = REPO_ROOT / "tools" / "memory_guard_stream.py"


def _load_stream_tool():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_memory_guard_stream", STREAM_TOOL
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _write_jsonl(path: Path, rows: list[dict[str, object]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(row, sort_keys=True) + "\n")


def test_stream_paths_include_bounded_rotations(tmp_path: Path) -> None:
    module = _load_stream_tool()
    guard_root = tmp_path / "memory_guard"

    paths = module.stream_paths(guard_root)

    assert paths == [
        guard_root / "events.jsonl.1",
        guard_root / "events.jsonl",
        guard_root / "global_samples.jsonl.1",
        guard_root / "global_samples.jsonl",
    ]


def test_read_history_merges_rotated_guard_streams(tmp_path: Path) -> None:
    module = _load_stream_tool()
    guard_root = tmp_path / "memory_guard"
    _write_jsonl(
        guard_root / "global_samples.jsonl.1",
        [{"ts": 1.0, "event": "sample", "total_gb": 1.0, "active_roots": []}],
    )
    _write_jsonl(
        guard_root / "events.jsonl",
        [{"ts": 2.0, "event": "guard_tripped", "message": "limit"}],
    )

    records = module.read_history(module.stream_paths(guard_root), limit=10)

    assert [record.payload["ts"] for record in records] == [1.0, 2.0]
    assert "sample total=1.00GB" in module.format_record(records[0])
    assert "TRIP limit" in module.format_record(records[1])
