#!/usr/bin/env python3
"""Summarize where `molt build` time went across many builds.

Set `MOLT_BUILD_DIAGNOSTICS=1` and `MOLT_BUILD_DIAGNOSTICS_FILE=<dir>/` (a
trailing separator selects one file per build) for any harness, for example
`tests/molt_diff.py`, then run:

    python3 tools/build_phase_report.py <dir> [--top N] [--json]
"""

from __future__ import annotations

import argparse
from collections.abc import Iterable
import json
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True, slots=True)
class BuildRecord:
    """One build's diagnostics: what it built and where its time went."""

    entry: str
    total_s: float
    phase_s: dict[str, float]
    module_graph_s: dict[str, float]

    @property
    def top_phase(self) -> str:
        return max(self.phase_s, key=self.phase_s.__getitem__, default="")


def load_records(directory: Path) -> list[BuildRecord]:
    """Every build diagnostics file in `directory`; a malformed one fails."""
    records = []
    for path in sorted(directory.glob("*.json")):
        payload = json.loads(path.read_text(encoding="utf-8"))
        entry = payload.get("entry") or {}
        phases = payload.get("phase_sec")
        total = payload.get("total_sec")
        graph = payload.get("module_graph_stage_sec", {})
        if (
            not isinstance(phases, dict)
            or not isinstance(total, (int, float))
            or not isinstance(graph, dict)
        ):
            raise ValueError(f"{path} is not a molt build diagnostics file")
        records.append(
            BuildRecord(
                entry=str(entry.get("path") or entry.get("module") or path.stem),
                total_s=float(total),
                phase_s={str(name): float(value) for name, value in phases.items()},
                module_graph_s={
                    str(name): float(value) for name, value in graph.items()
                },
            )
        )
    return records


@dataclass(frozen=True, slots=True)
class PhaseTotal:
    phase: str
    total_s: float
    share: float


@dataclass(frozen=True, slots=True)
class BuildPhaseSummary:
    builds: int
    total_s: float
    phases: tuple[PhaseTotal, ...]
    module_graph_stages: tuple[PhaseTotal, ...]
    slowest: tuple[BuildRecord, ...]

    @staticmethod
    def _phase_rows(phases: tuple[PhaseTotal, ...]) -> list[dict[str, object]]:
        return [
            {"phase": row.phase, "total_s": round(row.total_s, 3), "share": row.share}
            for row in phases
        ]

    def as_json(self) -> dict[str, object]:
        return {
            "builds": self.builds,
            "total_s": round(self.total_s, 3),
            "mean_s": round(self.total_s / self.builds, 3) if self.builds else 0.0,
            "phases": self._phase_rows(self.phases),
            "module_graph_stages": self._phase_rows(self.module_graph_stages),
            "slowest": [
                {
                    "entry": record.entry,
                    "total_s": round(record.total_s, 3),
                    "top_phase": record.top_phase,
                    "top_phase_s": round(record.phase_s.get(record.top_phase, 0.0), 3),
                }
                for record in self.slowest
            ],
        }

    def render(self) -> str:
        mean = self.total_s / self.builds if self.builds else 0.0
        lines = [
            f"builds={self.builds} total={self.total_s:.1f}s mean={mean:.1f}s",
            f"{'phase':<32} {'total_s':>9} {'share':>7}",
            *(
                f"{row.phase:<32} {row.total_s:>9.1f} {row.share:>7.1%}"
                for row in self.phases
            ),
        ]
        if self.module_graph_stages:
            lines.append("module graph stages (share of measured module graph time)")
            lines.extend(
                f"{row.phase:<32} {row.total_s:>9.1f} {row.share:>7.1%}"
                for row in self.module_graph_stages
            )
        lines.append("slowest builds:")
        lines.extend(
            f"  {record.total_s:>7.1f}s  {record.top_phase} "
            f"{record.phase_s.get(record.top_phase, 0.0):.1f}s  {record.entry}"
            for record in self.slowest
        )
        return "\n".join(lines)


def _totals(
    rows: Iterable[dict[str, float]], denominator: float
) -> tuple[PhaseTotal, ...]:
    totals: dict[str, float] = {}
    for row in rows:
        for name, seconds in row.items():
            totals[name] = totals.get(name, 0.0) + seconds
    return tuple(
        PhaseTotal(
            name, seconds, round(seconds / denominator, 4) if denominator else 0.0
        )
        for name, seconds in sorted(totals.items(), key=lambda item: -item[1])
    )


def summarize(records: list[BuildRecord], *, top: int) -> BuildPhaseSummary:
    total = sum(record.total_s for record in records)
    graph_records = [record for record in records if record.module_graph_s]
    return BuildPhaseSummary(
        builds=len(records),
        total_s=total,
        phases=_totals((record.phase_s for record in records), total),
        module_graph_stages=_totals(
            (record.module_graph_s for record in graph_records),
            sum(record.phase_s.get("module_graph", 0.0) for record in graph_records),
        ),
        slowest=tuple(sorted(records, key=lambda record: -record.total_s)[:top]),
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("directory", type=Path)
    parser.add_argument("--top", type=int, default=10)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    records = load_records(args.directory)
    if not records:
        print(f"no build diagnostics in {args.directory}", file=sys.stderr)
        return 1
    summary = summarize(records, top=args.top)
    print(json.dumps(summary.as_json(), indent=2) if args.json else summary.render())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
