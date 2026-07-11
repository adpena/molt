#!/usr/bin/env python3
"""Fail-open Stop hook leg that persists session cruxes into the memory graph."""

from __future__ import annotations

import datetime as dt
import json
import re
from pathlib import Path
from typing import Any, Iterable

from tools import memory_graph
from tools.hooks import _common, landing_gate

CRUX_RE = re.compile(r"(?i)\b(?:crux|root cause|recurring class|structural lesson)\b")
FRONTIER_RE = re.compile(
    r"(?i)\b(?:open frontier|new frontier|next frontier|blocker)\b"
)


def _strings(value: Any) -> Iterable[str]:
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for child in value.values():
            yield from _strings(child)
    elif isinstance(value, list):
        for child in value:
            yield from _strings(child)


def extract_learning(transcript_path: Path | None) -> tuple[list[str], list[str]]:
    cruxes: list[str] = []
    frontiers: list[str] = []
    if transcript_path is None or not transcript_path.is_file():
        return cruxes, frontiers
    try:
        lines = transcript_path.read_text(
            encoding="utf-8", errors="replace"
        ).splitlines()
    except OSError:
        return cruxes, frontiers
    for raw in lines[-500:]:
        try:
            payload = json.loads(raw)
        except json.JSONDecodeError:
            continue
        for text in _strings(payload):
            for line in text.splitlines():
                clean = " ".join(line.strip().split())
                if not clean or len(clean) > 500:
                    continue
                if CRUX_RE.search(clean) and clean not in cruxes:
                    cruxes.append(clean)
                if FRONTIER_RE.search(clean) and clean not in frontiers:
                    frontiers.append(clean)
    return cruxes[-8:], frontiers[-8:]


def render_digest(
    session_id: str,
    head: str,
    landings: list[str],
    cruxes: list[str],
    frontiers: list[str],
) -> str:
    def section(title: str, items: list[str]) -> list[str]:
        return [
            f"## {title}",
            *(f"- {item}" for item in items or ["none captured"]),
            "",
        ]

    lines = [f"# Session learning {session_id}", "", f"- head: {head or 'unknown'}", ""]
    lines += section("Landings", landings)
    lines += section("Crux learnings", cruxes)
    lines += section("Open frontiers", frontiers)
    return "\n".join(lines)


def record(data: dict[str, Any], root: Path) -> Path | None:
    memory_dir = memory_graph.discover_memory_dir(
        repo_root=root, cwd=str(data.get("cwd", ""))
    )
    if memory_dir is None:
        return None
    marker = _common.read_window_marker(root, landing_gate.MARKER_NAME)
    base = (
        marker.get("start_head") if isinstance(marker.get("start_head"), str) else None
    )
    landings = _common.git_window_messages(root, base)
    cruxes, frontiers = extract_learning(
        Path(str(data["transcript_path"])) if data.get("transcript_path") else None
    )
    if not landings and not cruxes and not frontiers:
        return None
    session_id = str(data.get("session_id", "unknown"))
    head = _common.git_head(root) or ""
    stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    target = memory_dir / "session_digests" / f"{stamp}-{session_id}.md"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(
        render_digest(session_id, head, landings, cruxes, frontiers), encoding="utf-8"
    )
    return target


def self_test() -> bool:
    text = render_digest(
        "s1", "abc", ["landed x"], ["root cause: y"], ["next frontier: z"]
    )
    return (
        "## Crux learnings" in text
        and "root cause: y" in text
        and "next frontier: z" in text
    )
