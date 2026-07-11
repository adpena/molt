#!/usr/bin/env python3
"""Variant-II acceptance for real, serial, release-profile performance evidence."""

from __future__ import annotations

import argparse
import json
import math
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Mapping


@dataclass(frozen=True)
class CorrectnessDemonstration:
    real_authority: bool
    release_profile: bool
    serial_differential: bool
    held_benches_pass: bool
    memory_ceiling_pass: bool
    sample_count: int


@dataclass(frozen=True)
class Acceptance:
    accepted: bool
    hard_errors: tuple[str, ...]
    advisories: tuple[str, ...]
    speedup: float | None


def variant_ii_accept(
    *,
    before: float | None,
    after: float | None,
    demonstration: CorrectnessDemonstration,
) -> Acceptance:
    hard: list[str] = []
    advisory: list[str] = []
    if (
        before is None
        or after is None
        or not all(math.isfinite(v) and v > 0 for v in (before, after))
    ):
        hard.append("finite positive before/after measurements required")
        speedup = None
    else:
        speedup = before / after
        if speedup <= 1.0:
            hard.append("measured net improvement required")
    if not demonstration.real_authority:
        hard.append("proxy evidence refused: real authority required")
    if not demonstration.release_profile:
        hard.append("dev-profile evidence refused")
    if not demonstration.serial_differential:
        hard.append("serial differential required")
    if demonstration.sample_count < 3:
        hard.append("single-run noise refused; at least 3 samples required")
    if not demonstration.held_benches_pass:
        hard.append("held-bench never-regress proof required")
    if not demonstration.memory_ceiling_pass:
        hard.append("memory-ceiling runnability required")
    return Acceptance(not hard, tuple(hard), tuple(advisory), speedup)


def _median(shape: object) -> float | None:
    if isinstance(shape, Mapping):
        for key in (
            "median_wall_seconds",
            "steady_state_wall_seconds",
            "median_ms",
            "wall_seconds",
        ):
            value = shape.get(key)
            if isinstance(value, (int, float)):
                return float(value)
    return None


def validate_attestation(payload: Mapping[str, object]) -> Acceptance:
    scenario = str(payload.get("scenario", "")).lower()
    before_shape = payload.get("before", payload.get("before_ms"))
    after_shape = payload.get("after", payload.get("after_ms"))
    before = _median(before_shape)
    after = _median(after_shape)
    if before is None and isinstance(payload.get("median_before_ms"), (int, float)):
        before = float(payload["median_before_ms"])
    if after is None and isinstance(payload.get("median_after_ms"), (int, float)):
        after = float(payload["median_after_ms"])

    def runs(shape: object) -> int:
        if isinstance(shape, Mapping) and isinstance(shape.get("runs"), list):
            return len(shape["runs"])
        if isinstance(shape, list):
            return len(shape)
        if isinstance(shape, Mapping) and isinstance(shape.get("wall_seconds"), list):
            return len(shape["wall_seconds"])
        return 1 if _median(shape) is not None else 0

    profile_text = json.dumps(payload.get("profile", "")).lower() + " " + scenario
    demo = CorrectnessDemonstration(
        real_authority=any(
            word in scenario for word in ("real ", "witness", "acceptance", "seal")
        ),
        release_profile="release" in profile_text and "dev-fast" not in profile_text,
        serial_differential="parallel" not in scenario,
        held_benches_pass=bool(payload.get("held_benches")),
        memory_ceiling_pass=bool(payload.get("memory_ceiling")),
        sample_count=min(runs(before_shape), runs(after_shape)),
    )
    return variant_ii_accept(before=before, after=after, demonstration=demo)


def rank_backlog(markdown: str) -> list[tuple[str, float]]:
    rows: list[tuple[str, float]] = []
    for line in markdown.splitlines():
        match = re.match(r"^\|\s*([^|]+)\|\s*([0-9.]+)\s*\|\s*([0-9.]+)\s*\|", line)
        if match and float(match.group(3)) > 0:
            rows.append(
                (match.group(1).strip(), float(match.group(2)) / float(match.group(3)))
            )
    return sorted(rows, key=lambda item: (-item[1], item[0]))


def validate_files(paths: Iterable[Path]) -> tuple[list[str], list[str]]:
    hard: list[str] = []
    advisory: list[str] = []
    for path in paths:
        result = validate_attestation(json.loads(path.read_text(encoding="utf-8")))
        prefix = path.as_posix()
        hard.extend(f"{prefix}: {item}" for item in result.hard_errors)
        if not result.accepted:
            advisory.append(
                f"{prefix}: not Variant-II citable until the missing evidence is recorded"
            )
    return hard, advisory


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("paths", nargs="+", type=Path)
    ap.add_argument("--strict", action="store_true")
    args = ap.parse_args(argv)
    hard, advisory = validate_files(args.paths)
    for item in hard:
        print("HARD " + item)
    for item in advisory:
        print("ADVISORY " + item)
    return 1 if args.strict and hard else 0


if __name__ == "__main__":
    raise SystemExit(main())
