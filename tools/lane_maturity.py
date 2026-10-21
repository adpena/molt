#!/usr/bin/env python3
"""Minimal lane-maturity authority for proof admission and lane completion.

The maturity records live in ``.molt/state/lane_registry.json``. They hold no
build-state custody: completing a lane marks its record and retires nothing.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Sequence

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt.custody_layout import git_directories  # noqa: E402

LEVELS = tuple(f"L{i}" for i in range(8))
REQUIRED_EVIDENCE = {
    "L0": (),
    "L1": ("unit_tested",),
    "L2": ("unit_tested", "native_e2e"),
    "L3": ("unit_tested", "native_e2e", "artifact_hash"),
    "L4": ("unit_tested", "native_e2e", "artifact_hash", "exact_parity"),
    "L5": (
        "unit_tested",
        "native_e2e",
        "artifact_hash",
        "exact_parity",
        "perf_attestation",
    ),
    "L6": (
        "unit_tested",
        "native_e2e",
        "artifact_hash",
        "exact_parity",
        "perf_attestation",
        "composable",
    ),
    "L7": (
        "unit_tested",
        "native_e2e",
        "artifact_hash",
        "exact_parity",
        "perf_attestation",
        "composable",
        "windows_wasm_parity",
        "python_312_matrix",
    ),
}
WASM_FAMILIES = frozenset({"wasm", "wasm-browser"})


@dataclass(frozen=True)
class Decision:
    allow: bool
    required: str
    actual: str
    reason: str


def decide(
    *,
    maturity: str,
    resource_family: str,
    cross_lane: bool = False,
    promotion: bool = False,
) -> Decision:
    required = (
        "L7"
        if promotion
        else "L4"
        if cross_lane
        else "L1"
        if resource_family.lower() in WASM_FAMILIES
        else "L0"
    )
    actual = maturity if maturity in LEVELS else "L0"
    allow = LEVELS.index(actual) >= LEVELS.index(required)
    return Decision(
        allow,
        required,
        actual,
        "admitted"
        if allow
        else f"{resource_family} proof requires {required}, lane is {actual}",
    )


def decide_transition(*, target: str, evidence: Sequence[str]) -> Decision:
    if target not in LEVELS:
        return Decision(False, target, "invalid", f"unknown maturity {target}")
    missing = sorted(set(REQUIRED_EVIDENCE[target]) - set(evidence))
    return Decision(
        not missing,
        target,
        target if not missing else "incomplete",
        "admitted" if not missing else "missing evidence: " + ", ".join(missing),
    )


def registry_owner_root(repo_root: Path) -> Path:
    directories = git_directories(repo_root)
    if directories is not None and directories[1].name == ".git":
        return directories[1].parent
    return repo_root


def registry_path(repo_root: Path) -> Path:
    return registry_owner_root(repo_root) / ".molt" / "state" / "lane_registry.json"


def read_registry(repo_root: Path) -> dict[str, dict]:
    path = registry_path(repo_root)
    if not path.exists():
        return {}
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise ValueError("lane registry must be an object")
    return {str(k): dict(v) for k, v in payload.items() if isinstance(v, dict)}


def write_registry(repo_root: Path, records: Mapping[str, dict]) -> None:
    path = registry_path(repo_root)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(
        json.dumps(dict(sorted(records.items())), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    os.replace(tmp, path)


def admission_check(
    *,
    repo_root: Path,
    lane_id: str,
    resource_family: str,
    cross_lane: bool = False,
    promotion: bool = False,
) -> Decision:
    """Fail-open wrapper: registry faults are loud but never block proof work."""
    try:
        records = read_registry(repo_root)
        record = records.get(lane_id)
        if record is None:
            record = {
                "maturity": "L0",
                "status": "active",
                "worktree": str(repo_root.resolve()),
                "updated_at": time.time(),
            }
            records[lane_id] = record
            write_registry(repo_root, records)
        return decide(
            maturity=str(record.get("maturity", "L0")),
            resource_family=resource_family,
            cross_lane=cross_lane,
            promotion=promotion,
        )
    except Exception as exc:
        print(f"lane_maturity: LOUD fail-open: {exc}", file=sys.stderr)
        return Decision(True, "unknown", "unknown", "registry unavailable; fail-open")


def complete_lane(*, repo_root: Path, lane_id: str) -> bool:
    """Mark ``lane_id`` completed; False when it has no record.

    Completion retires no build state: the retention policy of
    `molt.checkout_state` retires a lane's targets by last use, and `molt_dev
    lane retire` reclaims its checkout's state with the worktree.
    """
    records = read_registry(repo_root)
    record = records.get(lane_id)
    if record is None:
        return False
    record["status"] = "completed"
    record["completed_at"] = time.time()
    write_registry(repo_root, records)
    return True


def complete_worktree_lanes(*, repo_root: Path, worktree: Path) -> list[str]:
    records = read_registry(repo_root)
    wanted = os.path.normcase(str(worktree.resolve()))
    matched = [
        lane
        for lane, rec in records.items()
        if os.path.normcase(str(Path(str(rec.get("worktree", ""))).resolve())) == wanted
        and rec.get("status") == "active"
    ]
    for lane in matched:
        complete_lane(repo_root=repo_root, lane_id=lane)
    return matched


def complete_worktree_lanes_fail_open(*, repo_root: Path, worktree: Path) -> list[str]:
    try:
        return complete_worktree_lanes(repo_root=repo_root, worktree=worktree)
    except Exception as exc:
        print(f"lane_maturity completion LOUD fail-open: {exc}", file=sys.stderr)
        return []


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--repo-root", default=".")
    sub = ap.add_subparsers(dest="cmd", required=True)
    set_p = sub.add_parser("set")
    set_p.add_argument("lane")
    set_p.add_argument("maturity", choices=LEVELS)
    set_p.add_argument("--evidence", action="append", default=[])
    done_p = sub.add_parser("complete")
    done_p.add_argument("lane")
    args = ap.parse_args(argv)
    root = Path(args.repo_root).resolve()
    if args.cmd == "complete":
        complete_lane(repo_root=root, lane_id=args.lane)
        return 0
    decision = decide_transition(target=args.maturity, evidence=args.evidence)
    if not decision.allow:
        print(decision.reason, file=sys.stderr)
        return 2
    records = read_registry(root)
    record = records.setdefault(args.lane, {"worktree": str(root), "status": "active"})
    record.update(
        {
            "maturity": args.maturity,
            "evidence": sorted(set(args.evidence)),
            "updated_at": time.time(),
        }
    )
    write_registry(root, records)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
