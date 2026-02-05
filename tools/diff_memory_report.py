#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path


def _load_jsonl(path: Path) -> list[dict]:
    entries: list[dict] = []
    if not path.exists():
        return entries
    for line in path.read_text().splitlines():
        if not line.strip():
            continue
        try:
            entries.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return entries


def _select_run(entries: list[dict], run_id: str | None) -> list[dict]:
    if not run_id:
        return entries
    return [entry for entry in entries if entry.get("run_id") == run_id]


def _metric(entry: dict, phase: str, key: str) -> int:
    block = entry.get(phase) or {}
    if isinstance(block, dict):
        value = block.get(key)
        if isinstance(value, int):
            return value
    return 0


def _format_kb(value: int) -> str:
    if value <= 0:
        return "-"
    return f"{value} KB"


def main() -> int:
    parser = argparse.ArgumentParser(description="Summarize molt_diff RSS metrics.")
    parser.add_argument(
        "--input",
        default="",
        help="Path to rss_metrics.jsonl (default: MOLT_DIFF_ROOT/rss_metrics.jsonl).",
    )
    parser.add_argument(
        "--run-id",
        default="",
        help="Filter by run_id (matches summary.json).",
    )
    parser.add_argument(
        "--top",
        type=int,
        default=10,
        help="Number of top entries to show.",
    )
    args = parser.parse_args()

    input_path = (
        Path(args.input) if args.input else Path("logs/molt_diff/rss_metrics.jsonl")
    )
    entries = _load_jsonl(input_path)
    entries = _select_run(entries, args.run_id or None)
    if not entries:
        print("No RSS metrics found.")
        return 1

    ranked = sorted(entries, key=lambda e: _metric(e, "run", "max_rss"), reverse=True)
    top = ranked[: max(1, args.top)]
    print(f"Top {len(top)} RSS offenders (run phase):")
    for entry in top:
        file_path = entry.get("file", "<unknown>")
        rss = _metric(entry, "run", "max_rss")
        build_rss = _metric(entry, "build", "max_rss")
        status = entry.get("status", "")
        print(
            f"- {file_path} | run={_format_kb(rss)} build={_format_kb(build_rss)} status={status}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
