#!/usr/bin/env python3
"""Registered DAG/DSL/equations agreement authority (APPARATUS TRIALITY)."""

from __future__ import annotations
import argparse
import hashlib
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

REGISTRY = Path(".molt/state/triality_registry.json")
LEGS = ("dag", "dsl", "equations")


@dataclass(frozen=True)
class TrialityVerdict:
    finding_id: str
    known: bool
    reasons: tuple[str, ...]


def invariant_fingerprint(invariant: str) -> str:
    return hashlib.sha256(" ".join(invariant.split()).encode()).hexdigest()[:16]


def decide(entry: Mapping[str, object]) -> TrialityVerdict:
    finding_id = str(entry.get("finding_id", "<missing>"))
    invariant = str(entry.get("invariant", "")).strip()
    expected = invariant_fingerprint(invariant) if invariant else ""
    reasons: list[str] = []
    if not invariant:
        reasons.append("missing invariant")
    legs = entry.get("legs")
    if not isinstance(legs, Mapping):
        return TrialityVerdict(
            finding_id, False, tuple(reasons + ["missing legs mapping"])
        )
    for leg in LEGS:
        value = legs.get(leg)
        if not isinstance(value, Mapping):
            reasons.append(f"missing {leg} leg")
            continue
        if not str(value.get("authority", "")).strip():
            reasons.append(f"{leg} leg lacks authority")
        if str(value.get("fingerprint", "")) != expected:
            reasons.append(f"{leg} leg disagrees with invariant")
    return TrialityVerdict(finding_id, not reasons, tuple(reasons))


def audit(registry: Mapping[str, object]) -> list[TrialityVerdict]:
    entries = registry.get("findings", [])
    if not isinstance(entries, list):
        return [TrialityVerdict("<registry>", False, ("findings must be a list",))]
    return [decide(entry) for entry in entries if isinstance(entry, Mapping)]


def load(path: Path = REGISTRY) -> Mapping[str, object]:
    return json.loads(path.read_text(encoding="utf-8"))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--registry", type=Path, default=REGISTRY)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)
    try:
        verdicts = audit(load(args.registry))
    except Exception as exc:
        print(f"triality: advisory unavailable: {exc}", file=sys.stderr)
        return 0
    drift = [v for v in verdicts if not v.known]
    for verdict in verdicts:
        state = "KNOWN" if verdict.known else "DRIFT"
        print(
            f"{state} {verdict.finding_id}"
            + (f": {'; '.join(verdict.reasons)}" if verdict.reasons else "")
        )
    return 1 if args.check and drift else 0


if __name__ == "__main__":
    raise SystemExit(main())
