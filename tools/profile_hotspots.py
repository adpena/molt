#!/usr/bin/env python3
"""Summarize default guarded-command hotspot telemetry."""

from __future__ import annotations

import argparse
from collections import Counter, defaultdict
from collections.abc import Iterable, Mapping, Sequence
import json
import os
import shlex
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools import harness_memory_guard  # noqa: E402


def _command_text(command: object) -> str:
    if isinstance(command, list):
        return " ".join(shlex.quote(str(part)) for part in command)
    if isinstance(command, tuple):
        return " ".join(shlex.quote(str(part)) for part in command)
    return str(command)


def _short_command(command: object, *, max_parts: int = 8) -> str:
    if not isinstance(command, list | tuple):
        text = str(command)
        return text if len(text) <= 160 else text[:157] + "..."
    parts = [str(part) for part in command]
    visible = parts[:max_parts]
    suffix = " ..." if len(parts) > max_parts else ""
    text = " ".join(shlex.quote(part) for part in visible) + suffix
    return text if len(text) <= 160 else text[:157] + "..."


def _read_jsonl(path: Path) -> Iterable[dict[str, Any]]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except FileNotFoundError:
        return
    for lineno, line in enumerate(lines, start=1):
        stripped = line.strip()
        if not stripped:
            continue
        try:
            payload = json.loads(stripped)
        except json.JSONDecodeError as exc:
            raise ValueError(f"{path}:{lineno}: invalid JSONL: {exc}") from exc
        if isinstance(payload, dict):
            yield payload


def load_events(paths: Sequence[Path]) -> list[dict[str, Any]]:
    events: list[dict[str, Any]] = []
    for path in paths:
        for payload in _read_jsonl(path):
            if payload.get("event") == "guarded_command_profile":
                events.append(payload)
    return events


def _elapsed(event: Mapping[str, Any]) -> float:
    raw = event.get("elapsed_s")
    return raw if isinstance(raw, int | float) else 0.0


def _matches_filters(
    event: Mapping[str, Any],
    *,
    min_elapsed_s: float,
    prefix: str | None,
) -> bool:
    if _elapsed(event) < min_elapsed_s:
        return False
    if prefix is not None and str(event.get("prefix", "")).upper() != prefix.upper():
        return False
    return True


def summarize_events(
    events: Sequence[dict[str, Any]],
    *,
    limit: int,
    min_elapsed_s: float = 0.0,
    prefix: str | None = None,
) -> dict[str, Any]:
    filtered = [
        event
        for event in events
        if _matches_filters(event, min_elapsed_s=min_elapsed_s, prefix=prefix)
    ]
    sorted_events = sorted(filtered, key=_elapsed, reverse=True)
    by_command: dict[str, dict[str, Any]] = defaultdict(
        lambda: {
            "command": "",
            "count": 0,
            "total_elapsed_s": 0.0,
            "max_elapsed_s": 0.0,
            "statuses": Counter(),
            "prefixes": Counter(),
        }
    )
    status_counts: Counter[str] = Counter()
    prefix_counts: Counter[str] = Counter()
    for event in filtered:
        elapsed = _elapsed(event)
        status = str(event.get("status", "unknown"))
        prefix_name = str(event.get("prefix", ""))
        command_text = _command_text(event.get("command", ""))
        bucket = by_command[command_text]
        bucket["command"] = command_text
        bucket["count"] += 1
        bucket["total_elapsed_s"] += elapsed
        bucket["max_elapsed_s"] = max(float(bucket["max_elapsed_s"]), elapsed)
        bucket["statuses"][status] += 1
        bucket["prefixes"][prefix_name] += 1
        status_counts[status] += 1
        prefix_counts[prefix_name] += 1

    grouped = sorted(
        by_command.values(),
        key=lambda item: (float(item["total_elapsed_s"]), float(item["max_elapsed_s"])),
        reverse=True,
    )
    return {
        "schema_version": "1.0",
        "total_events": len(events),
        "filtered_events": len(filtered),
        "total_elapsed_s": round(sum(_elapsed(event) for event in filtered), 6),
        "slowest_events": [
            {
                "elapsed_s": event.get("elapsed_s"),
                "returncode": event.get("returncode"),
                "status": event.get("status"),
                "prefix": event.get("prefix"),
                "recorded_at": event.get("recorded_at"),
                "command": event.get("command"),
                "short_command": _short_command(event.get("command", "")),
            }
            for event in sorted_events[:limit]
        ],
        "slowest_commands": [
            {
                "command": item["command"],
                "short_command": _short_command(item["command"]),
                "count": item["count"],
                "total_elapsed_s": round(float(item["total_elapsed_s"]), 6),
                "max_elapsed_s": round(float(item["max_elapsed_s"]), 6),
                "statuses": dict(item["statuses"]),
                "prefixes": dict(item["prefixes"]),
            }
            for item in grouped[:limit]
        ],
        "status_counts": dict(status_counts),
        "prefix_counts": dict(prefix_counts),
    }


def _format_seconds(value: object) -> str:
    if isinstance(value, int | float):
        return f"{value:.2f}s"
    return "unknown"


def format_summary(summary: Mapping[str, Any]) -> str:
    lines: list[str] = []
    lines.append(
        "Guarded command hotspots: "
        f"{summary['filtered_events']} filtered / {summary['total_events']} total, "
        f"elapsed={_format_seconds(summary['total_elapsed_s'])}"
    )
    slowest_events = summary.get("slowest_events", [])
    if slowest_events:
        lines.append("")
        lines.append("Slowest events:")
        for index, event in enumerate(slowest_events, start=1):
            lines.append(
                f"  {index}. {_format_seconds(event.get('elapsed_s'))} "
                f"rc={event.get('returncode')} "
                f"status={event.get('status')} "
                f"prefix={event.get('prefix')} "
                f"{event.get('short_command')}"
            )
    slowest_commands = summary.get("slowest_commands", [])
    if slowest_commands:
        lines.append("")
        lines.append("Slowest grouped commands:")
        for index, item in enumerate(slowest_commands, start=1):
            lines.append(
                f"  {index}. total={_format_seconds(item.get('total_elapsed_s'))} "
                f"max={_format_seconds(item.get('max_elapsed_s'))} "
                f"runs={item.get('count')} "
                f"{item.get('short_command')}"
            )
    status_counts = summary.get("status_counts", {})
    if status_counts:
        lines.append("")
        lines.append(
            "Statuses: " + ", ".join(f"{k}={v}" for k, v in status_counts.items())
        )
    return "\n".join(lines)


def _parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Summarize slow guarded subprocesses from command-profile JSONL."
    )
    parser.add_argument(
        "--log",
        action="append",
        type=Path,
        default=None,
        help=(
            "JSONL command profile log to read. May be repeated. Defaults to "
            "the shared harness memory-guard profile log."
        ),
    )
    parser.add_argument("--limit", type=int, default=10)
    parser.add_argument("--min-elapsed-s", type=float, default=0.0)
    parser.add_argument("--prefix", default=None)
    parser.add_argument("--json", action="store_true")
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = _parse_args(argv)
    paths = args.log or [harness_memory_guard.command_profile_log_path(os.environ)]
    events = load_events(paths)
    summary = summarize_events(
        events,
        limit=max(1, args.limit),
        min_elapsed_s=max(0.0, args.min_elapsed_s),
        prefix=args.prefix,
    )
    if args.json:
        print(json.dumps(summary, indent=2, sort_keys=True))
    else:
        print(format_summary(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
