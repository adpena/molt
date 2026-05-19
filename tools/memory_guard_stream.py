#!/usr/bin/env python3
from __future__ import annotations

import argparse
from collections import deque
from collections.abc import Iterable, Iterator, Sequence
from dataclasses import dataclass
from datetime import datetime
import json
import os
from pathlib import Path
import sys
import time


@dataclass(frozen=True, slots=True)
class StreamRecord:
    path: Path
    payload: dict[str, object]


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def default_diff_root() -> Path:
    raw = os.environ.get("MOLT_DIFF_ROOT", "").strip()
    if raw:
        return Path(raw).expanduser()
    ext_root = os.environ.get("MOLT_EXT_ROOT", "").strip()
    if ext_root:
        return Path(ext_root).expanduser() / "tmp" / "diff"
    return repo_root() / "tmp" / "diff"


def guard_root_from_args(args: argparse.Namespace) -> Path:
    if args.guard_root:
        return Path(args.guard_root).expanduser()
    return Path(args.diff_root).expanduser() / "memory_guard"


def stream_paths(guard_root: Path) -> list[Path]:
    event = guard_root / "events.jsonl"
    samples = guard_root / "global_samples.jsonl"
    return [
        event.with_name(f"{event.name}.1"),
        event,
        samples.with_name(f"{samples.name}.1"),
        samples,
    ]


def _read_jsonl(path: Path) -> Iterator[StreamRecord]:
    try:
        with path.open("r", encoding="utf-8") as handle:
            for line in handle:
                stripped = line.strip()
                if not stripped:
                    continue
                try:
                    payload = json.loads(stripped)
                except json.JSONDecodeError:
                    continue
                if isinstance(payload, dict):
                    yield StreamRecord(path=path, payload=payload)
    except FileNotFoundError:
        return


def read_history(paths: Iterable[Path], limit: int) -> list[StreamRecord]:
    records: deque[StreamRecord] = deque(maxlen=max(0, limit))
    for path in paths:
        records.extend(_read_jsonl(path))
    return sorted(records, key=lambda record: float(record.payload.get("ts", 0.0)))


def _format_ts(payload: dict[str, object]) -> str:
    raw = payload.get("ts")
    if isinstance(raw, (int, float)):
        return datetime.fromtimestamp(raw).strftime("%H:%M:%S")
    return "--:--:--"


def _gb(value: object) -> str:
    if isinstance(value, (int, float)):
        return f"{value:.2f}GB"
    return "-"


def _record_gb(record: object) -> str:
    if isinstance(record, dict):
        return _gb(record.get("rss_gb"))
    return "-"


def _sample_peak_tree_gb(payload: dict[str, object]) -> str:
    trees = payload.get("trees")
    if not isinstance(trees, list):
        return "-"
    totals: list[float] = []
    for tree in trees:
        if not isinstance(tree, dict):
            continue
        total = tree.get("total")
        if not isinstance(total, dict):
            continue
        raw = total.get("rss_gb")
        if isinstance(raw, (int, float)):
            totals.append(float(raw))
    if not totals:
        return "-"
    return f"{max(totals):.2f}GB"


def format_record(record: StreamRecord) -> str:
    payload = record.payload
    event = str(payload.get("event", "sample"))
    ts = _format_ts(payload)
    if event == "sample":
        roots = payload.get("active_roots")
        root_count = len(roots) if isinstance(roots, list) else 0
        return (
            f"{ts} sample total={_gb(payload.get('total_gb'))} "
            f"roots={root_count} peak_tree={_sample_peak_tree_gb(payload)}"
        )
    if event in {"guard_tripped", "subprocess_guard_tripped", "memory_guard_trip"}:
        message = payload.get("message")
        violation = payload.get("violation")
        if isinstance(message, str) and message:
            return f"{ts} TRIP {message}"
        return f"{ts} TRIP violation={_record_gb(violation)}"
    if event == "run_started":
        return (
            f"{ts} run_started global={_gb(payload.get('global_gb'))} "
            f"tree={_gb(payload.get('max_tree_gb'))} "
            f"process={_gb(payload.get('max_process_gb'))} "
            f"poll={payload.get('poll_interval')}s"
        )
    if event == "monitor_error":
        return f"{ts} monitor_error {payload.get('error', '')}"
    return f"{ts} {event} {json.dumps(payload, sort_keys=True)}"


def emit_record(record: StreamRecord, *, json_mode: bool) -> None:
    if json_mode:
        print(json.dumps(record.payload, sort_keys=True), flush=True)
    else:
        print(format_record(record), flush=True)


class JsonlFollower:
    def __init__(self, paths: Sequence[Path], *, from_start: bool) -> None:
        self._paths = list(paths)
        self._offsets: dict[Path, int] = {}
        if not from_start:
            for path in self._paths:
                try:
                    self._offsets[path] = path.stat().st_size
                except FileNotFoundError:
                    self._offsets[path] = 0

    def poll(self) -> list[StreamRecord]:
        records: list[StreamRecord] = []
        for path in self._paths:
            offset = self._offsets.get(path, 0)
            try:
                size = path.stat().st_size
            except FileNotFoundError:
                self._offsets[path] = 0
                continue
            if size < offset:
                offset = 0
            if size == offset:
                self._offsets[path] = offset
                continue
            with path.open("r", encoding="utf-8") as handle:
                handle.seek(offset)
                for line in handle:
                    stripped = line.strip()
                    if not stripped:
                        continue
                    try:
                        payload = json.loads(stripped)
                    except json.JSONDecodeError:
                        continue
                    if isinstance(payload, dict):
                        records.append(StreamRecord(path=path, payload=payload))
                self._offsets[path] = handle.tell()
        return sorted(records, key=lambda record: float(record.payload.get("ts", 0.0)))


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Tail molt_diff memory-guard telemetry without sampling processes."
    )
    parser.add_argument(
        "--diff-root",
        default=str(default_diff_root()),
        help="Differential artifact root containing memory_guard/.",
    )
    parser.add_argument(
        "--guard-root",
        help="Explicit memory_guard telemetry directory.",
    )
    parser.add_argument(
        "--history",
        type=int,
        default=20,
        help="Print this many historical records before following.",
    )
    parser.add_argument(
        "--interval",
        type=float,
        default=0.25,
        help="Follow poll interval in seconds.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit raw JSONL instead of a compact human-readable stream.",
    )
    parser.add_argument(
        "--once",
        action="store_true",
        help="Print history and exit.",
    )
    parser.add_argument(
        "--from-start",
        action="store_true",
        help="When following, stream existing files from the beginning.",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.interval <= 0:
        parser.error("--interval must be greater than 0")
    guard_root = guard_root_from_args(args)
    paths = stream_paths(guard_root)
    for record in read_history(paths, args.history):
        emit_record(record, json_mode=args.json)
    if args.once:
        return 0
    follower = JsonlFollower(paths, from_start=args.from_start)
    try:
        while True:
            for record in follower.poll():
                emit_record(record, json_mode=args.json)
            time.sleep(args.interval)
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
