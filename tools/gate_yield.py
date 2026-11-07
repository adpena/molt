#!/usr/bin/env python3
"""Gate yield: what each landing gate costs and what it has caught.

Reads the proof receipts the landing executor writes under the integration
evidence root (``molt.dx.storage_path(..., "integration-receipts")``) and
reports, per gate command, how often it ran, its wall and child CPU seconds,
its peak memory, and its failures split three ways:

* candidate: the gate exited nonzero and no retry of it on the same source
  commit passed;
* flake: it failed (nonzero or memory pressure) and a retry of the same gate
  on the same commit passed;
* cancelled: the executor stopped it after a sibling gate failed (no
  verdict, and no fault of its own);
* infrastructure: memory pressure, guard or runner faults without a
  passing retry.

A costly gate with no candidate failures over many landings is a postsubmit
candidate. Prior art: Google TAP and Meta's predictive test selection keep
this per-test record and demote costly tests that never catch a defect.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from dataclasses import dataclass, field
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)


@dataclass
class GateYield:
    command: str
    runs: int = 0
    seconds: float = 0.0
    cpu_seconds: float = 0.0
    peak_rss_bytes: int = 0
    candidate: int = 0
    flake: int = 0
    cancelled: int = 0
    infrastructure: int = 0
    commits: set[str] = field(default_factory=set)

    def record(self) -> dict[str, object]:
        return {
            "command": self.command,
            "runs": self.runs,
            "seconds": round(self.seconds, 1),
            "cpu_seconds": round(self.cpu_seconds, 1),
            "peak_rss_bytes": self.peak_rss_bytes,
            "candidate_failures": self.candidate,
            "flakes": self.flake,
            "cancelled": self.cancelled,
            "infrastructure_failures": self.infrastructure,
            "commits": len(self.commits),
        }


def gate_command(argv: list[str]) -> str:
    """One gate's identity: its argv after the interpreter path."""
    parts = [str(part) for part in argv]
    if parts and Path(parts[0]).name.startswith("python"):
        parts[0] = "python"
    return " ".join(parts)


def _is_exit(reason: object) -> bool:
    return isinstance(reason, str) and reason.startswith("exit ")


def gate_yields(receipts: list[tuple[Path, dict]]) -> list[GateYield]:
    """Aggregate receipts, given as (path, payload), most costly first."""
    passed_retries: set[tuple[str, str]] = set()
    for path, payload in receipts:
        if "retry" not in path.name:
            continue
        for command in payload.get("commands", ()):
            if command.get("failure_reason") is None:
                key = gate_command(command.get("argv", ()))
                passed_retries.add((str(payload.get("source_commit")), key))
    gates: dict[str, GateYield] = {}
    for _path, payload in receipts:
        source = str(payload.get("source_commit"))
        for command in payload.get("commands", ()):
            key = gate_command(command.get("argv", ()))
            gate = gates.setdefault(key, GateYield(key))
            gate.runs += 1
            gate.seconds += float(command.get("duration_seconds") or 0.0)
            gate.cpu_seconds += float(command.get("child_cpu_seconds") or 0.0)
            gate.peak_rss_bytes = max(
                gate.peak_rss_bytes, int(command.get("peak_rss_bytes") or 0)
            )
            gate.commits.add(source)
            reason = command.get("failure_reason")
            if reason is None:
                continue
            retried = (source, key) in passed_retries
            if retried and (_is_exit(reason) or reason == "unsafe memory pressure"):
                gate.flake += 1
            elif _is_exit(reason):
                gate.candidate += 1
            elif reason == "executor cancellation":
                gate.cancelled += 1
            else:
                gate.infrastructure += 1
    return sorted(gates.values(), key=lambda gate: -gate.seconds)


def load_receipts(directory: Path) -> list[tuple[Path, dict]]:
    receipts = []
    for path in sorted(directory.glob("*.json")):
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if isinstance(payload, dict) and isinstance(payload.get("commands"), list):
            receipts.append((path, payload))
    return receipts


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument(
        "--evidence",
        type=Path,
        help="integration receipts directory (default: this checkout's)",
    )
    parser.add_argument("--top", type=int, default=25)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    directory = args.evidence
    if directory is None:
        from molt.dx import storage_path

        directory = storage_path(ROOT, "integration-receipts", os.environ)
    gates = gate_yields(load_receipts(directory))
    if args.json:
        print(json.dumps([gate.record() for gate in gates], indent=1))
        return 0
    total = sum(gate.seconds for gate in gates) or 1.0
    print(
        f"{'hours':>6} {'share':>6} {'runs':>5} {'cand':>4} {'flake':>5} "
        f"{'cancl':>5} {'infra':>5}  gate"
    )
    for gate in gates[: args.top]:
        print(
            f"{gate.seconds / 3600:6.1f} {100 * gate.seconds / total:5.1f}% "
            f"{gate.runs:5d} {gate.candidate:4d} {gate.flake:5d} "
            f"{gate.cancelled:5d} {gate.infrastructure:5d}  {gate.command[:110]}"
        )
    print(
        f"gate-yield: {len(gates)} gates, {total / 3600:.1f} gate-hours in {directory}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
