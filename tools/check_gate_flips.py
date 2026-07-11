#!/usr/bin/env python3
"""Gate warn->strict flip auditor (APPARATUS Wave 1, A9).

pact's lifecycle discipline: a gate lands WARN-ONLY with a NAMED, machine-
checkable flip condition, then is flipped to STRICT in a separate commit once
the condition is met. Left to prose, the flip is forgotten and gates rot
warn-only forever. This tool is "the ratchet that ratchets the ratchets": it
reads the ``[[gate_flip]]`` registry in ``tools/molt_dev_gates.toml`` and reports
any gate whose ``strict_when`` is MET but is still ``state = "warn"``.

``strict_when`` machine tokens:
  * ``live_count == 0`` / ``live_count <= N`` -- evaluated against ``count_cmd``
    (a shell command whose integer stdout is the live violation count).
  * ``landed_functional`` / ``always`` -- lifecycle markers for gates that land
    already-strict; nothing to flip.
  * anything else -- free text; reported as MANUAL REVIEW (never a hard fail).

Exit codes (``--check``): 0 = no met-but-unflipped gate; 1 = at least one warn
gate whose machine-checkable flip condition is met (drift). Without ``--check``
it only prints a report and exits 0.

Pure + stdlib-only; the flip-evaluation core (``evaluate_flip``) is unit-tested.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = ROOT / "tools" / "molt_dev_gates.toml"

_COND_RE = re.compile(r"^live_count\s*(==|<=|<|>=|>)\s*(\d+)$")


@dataclass
class FlipVerdict:
    name: str
    state: str
    strict_when: str
    status: str  # "ok-strict" | "should-flip" | "still-warn" | "manual" | "error"
    detail: str = ""


def _eval_condition(cond: str, live_count: int | None) -> bool | None:
    """True/False if the machine token is decidable, else None (manual)."""
    cond = cond.strip()
    if cond in ("landed_functional", "always"):
        return None  # lifecycle marker, not a flip trigger
    m = _COND_RE.match(cond)
    if not m:
        return None  # free text -> manual review
    if live_count is None:
        return None  # decidable form but no count source -> manual
    op, rhs = m.group(1), int(m.group(2))
    return {
        "==": live_count == rhs,
        "<=": live_count <= rhs,
        "<": live_count < rhs,
        ">=": live_count >= rhs,
        ">": live_count > rhs,
    }[op]


def _run_count_cmd(count_cmd: str, cwd: Path) -> int | None:
    if not count_cmd:
        return None
    try:
        r = subprocess.run(
            count_cmd,
            cwd=str(cwd),
            shell=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=60,
        )
        if r.returncode != 0:
            return None
        return int(r.stdout.strip())
    except Exception:
        return None


def evaluate_flip(entry: dict, *, live_count: int | None = None) -> FlipVerdict:
    """Pure: classify one ``[[gate_flip]]`` entry given an optional live count."""
    name = str(entry.get("name", "?"))
    state = str(entry.get("state", "warn")).lower()
    strict_when = str(entry.get("strict_when", ""))
    if state == "strict":
        return FlipVerdict(name, state, strict_when, "ok-strict")
    met = _eval_condition(strict_when, live_count)
    if met is None:
        return FlipVerdict(
            name,
            state,
            strict_when,
            "manual",
            "condition needs human evaluation (free text or no count source)",
        )
    if met:
        return FlipVerdict(
            name,
            state,
            strict_when,
            "should-flip",
            f"strict_when '{strict_when}' is MET (live_count={live_count}); flip to strict.",
        )
    return FlipVerdict(
        name,
        state,
        strict_when,
        "still-warn",
        f"strict_when '{strict_when}' not yet met (live_count={live_count}).",
    )


def load_flip_entries(config_path: Path) -> list[dict]:
    data = tomllib.loads(config_path.read_text(encoding="utf-8"))
    entries = data.get("gate_flip", [])
    return [e for e in entries if isinstance(e, dict)]


def audit(config_path: Path, cwd: Path) -> list[FlipVerdict]:
    verdicts: list[FlipVerdict] = []
    for entry in load_flip_entries(config_path):
        live_count = _run_count_cmd(str(entry.get("count_cmd", "")), cwd)
        verdicts.append(evaluate_flip(entry, live_count=live_count))
    return verdicts


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="check_gate_flips", description=__doc__)
    ap.add_argument("--config", default=str(DEFAULT_CONFIG))
    ap.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if any warn gate's flip condition is met (drift)",
    )
    args = ap.parse_args(argv)

    config_path = Path(args.config)
    try:
        verdicts = audit(config_path, config_path.resolve().parents[1])
    except Exception as exc:  # a broken registry should be loud, not silent
        sys.stderr.write(f"check_gate_flips: cannot read {config_path}: {exc}\n")
        return 2

    should_flip = [v for v in verdicts if v.status == "should-flip"]
    for v in verdicts:
        marker = {
            "ok-strict": "  [strict]",
            "still-warn": "  [warn]  ",
            "manual": "  [manual]",
            "should-flip": "  [FLIP!] ",
        }.get(v.status, "  [?]     ")
        line = f"{marker} {v.name}: {v.strict_when}"
        if v.detail:
            line += f" -- {v.detail}"
        print(line)

    if should_flip:
        print(
            f"\n{len(should_flip)} gate(s) have a MET flip condition but are still warn-only. "
            f'Flip them to state = "strict" in {config_path}.'
        )
        if args.check:
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
