#!/usr/bin/env python3
"""Durable board history + the board-vs-history regression gate (doc 64 §3.4 / Phase 4).

The absolute CPython floor (``perf_board``) answers "is this cell below CPython
NOW?". This module answers the *second* Performance-Constitution triage axis:
"was this cell GREEN before and is it RED now?" — i.e. a REGRESSION, not just an
absolute red. CLAUDE.md triage priority #2 ("any previously-green benchmark that
regressed") is only gateable if "previously-green" is a queryable FACT.

Design (doc 64 §3.4):
- ``board_identity = sha256(git_rev || benchmark_tool_blob || suite_hash ||
  host_class)`` — content-addressed identity so a regression is compared only
  against a board measured under the *same* tool + suite + host class (comparing
  a Windows board to a Linux board is meaningless).
- ``bench/scoreboard/history/<board>/<entry>.json`` + a per-board ``index.json``
  is the durable trail. Only AUTHORITATIVE boards seed the baseline (a noisy /
  dirty / non-quiescent board cannot become the reference, Rule 2).
- ``regressions_vs_history`` compares a candidate projected board against the most
  recent authoritative history entry of the SAME identity-class: a cell that was
  PASS in history and is FAIL now is a regression with ``severity=error``. A
  cell merely below baseline-but-still-passing is ``severity=warn`` (a drift
  signal, not a gate fail).

This module is a *consumer* of ``perf_board`` projections; it introduces no new
measurement. The regression gate uses ``perf_regression`` thresholds only as the
"is this a real delta or noise" filter (a within-CI delta is not a regression —
the no-false-positive gate of doc 64 §5).
"""

from __future__ import annotations

import datetime as dt
import hashlib
import json
from collections.abc import Mapping, Sequence
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
HISTORY_DIR = REPO_ROOT / "bench" / "scoreboard" / "history"

# A regression is only "real" if the candidate is below baseline by more than
# this fraction (mirrors perf_regression's slowdown sensitivity). A 5% drop that
# still passes the floor is a WARN drift; a flip from PASS to FAIL is always an
# ERROR regardless of magnitude.
DRIFT_WARN_FRACTION = 0.05


# ---------------------------------------------------------------------------
# Board identity
# ---------------------------------------------------------------------------


def _host_class(host: Mapping[str, Any]) -> str:
    """A coarse host equivalence class: platform + arch + pointer width + the
    CPython baseline version. Two boards are comparable only within one class."""
    return "|".join(
        str(host.get(k, "?"))
        for k in ("platform", "arch", "pointer_bits", "cpython_baseline")
    )


def _suite_hash(board: Mapping[str, Any]) -> str:
    """Content hash of the benchmark set this board covers (its cell keys), so a
    board that measured a different suite is a different identity-class."""
    table = board.get("table", {})
    keys = sorted(_iter_cell_keys(table))
    return hashlib.sha256("\n".join(keys).encode("utf-8")).hexdigest()[:16]


def _iter_cell_keys(node: Any) -> list[str]:
    """Walk a projected board ``table`` (nested by group_by) and yield leaf cell
    keys (``benchmark [backend/profile]``)."""
    out: list[str] = []
    if isinstance(node, Mapping):
        # A leaf is a cell dict (has a "gate" entry); otherwise recurse.
        if "gate" in node and "benchmark" in node:
            out.append(
                f"{node.get('benchmark')} [{node.get('backend')}/{node.get('profile')}]"
            )
        else:
            for child in node.values():
                out.extend(_iter_cell_keys(child))
    return out


def board_identity(board: Mapping[str, Any]) -> str:
    """``sha256(git_rev || benchmark_tool_blob || suite_hash || host_class)`` —
    content-addressed board identity (doc 64 §3.4)."""
    prov = board.get("provenance", {})
    tool_blob = (
        str(prov.get("benchmark_tool_sha", "?")) if isinstance(prov, Mapping) else "?"
    )
    parts = [
        str(board.get("git_rev", "?")),
        tool_blob,
        _suite_hash(board),
        _host_class(board.get("host", {})),
    ]
    return hashlib.sha256("||".join(parts).encode("utf-8")).hexdigest()


def identity_class(board: Mapping[str, Any]) -> str:
    """The identity-class a board is comparable WITHIN (tool + suite + host,
    *excluding* git_rev — two revs of the same tool/suite/host are comparable;
    that comparability is the whole point of a regression gate)."""
    prov = board.get("provenance", {})
    tool_blob = (
        str(prov.get("benchmark_tool_sha", "?")) if isinstance(prov, Mapping) else "?"
    )
    parts = [tool_blob, _suite_hash(board), _host_class(board.get("host", {}))]
    return hashlib.sha256("||".join(parts).encode("utf-8")).hexdigest()[:16]


# ---------------------------------------------------------------------------
# History store
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class HistoryEntry:
    board: str  # board name (cpython/backend/...)
    identity: str  # full board_identity
    identity_class: str  # comparability class
    git_rev: str
    generated_at: str
    authoritative: bool
    board_state: str
    path: str  # relative path of the stored board json


def _index_path(board_name: str, base: Path) -> Path:
    return base / board_name / "index.json"


def _load_index_at(board_name: str, base: Path) -> list[dict[str, Any]]:
    """Load a board's history index from an explicit history root."""
    p = _index_path(board_name, base)
    if not p.exists():
        return []
    try:
        data = json.loads(p.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return []
    entries = data.get("entries") if isinstance(data, Mapping) else None
    return list(entries) if isinstance(entries, list) else []


def _write_index_at(
    board_name: str, base: Path, entries: Sequence[Mapping[str, Any]]
) -> None:
    p = _index_path(board_name, base)
    p.parent.mkdir(parents=True, exist_ok=True)
    doc = {
        "board": board_name,
        "updated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "entries": list(entries),
    }
    tmp = p.with_suffix(".tmp")
    tmp.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    tmp.replace(p)


def record_board(
    board: Mapping[str, Any], *, history_dir: Path | None = None
) -> HistoryEntry:
    """Append a projected board to its history and update the per-board index.

    The board file is content-addressed by ``board_identity`` so re-recording the
    same measurement is idempotent. The index is the queryable trail the
    regression gate reads."""
    base = history_dir or HISTORY_DIR
    board_name = str(board.get("board"))
    if not board_name:
        raise ValueError("board has no 'board' name field; not a projected board")
    identity = board_identity(board)
    cls = identity_class(board)
    prov = board.get("provenance", {})
    authoritative = bool(
        prov.get("authoritative", board.get("source_authoritative", False))
        if isinstance(prov, Mapping)
        else False
    )
    git_rev = str(board.get("git_rev", "?"))
    generated_at = str(
        board.get("generated_at") or dt.datetime.now(dt.timezone.utc).isoformat()
    )
    state = str(board.get("summary", {}).get("board_state", "?"))

    board_dir = base / board_name
    board_dir.mkdir(parents=True, exist_ok=True)
    fname = f"{git_rev[:12]}_{identity[:12]}.json"
    fpath = board_dir / fname
    tmp = fpath.with_suffix(".tmp")
    tmp.write_text(json.dumps(board, indent=2) + "\n", encoding="utf-8")
    tmp.replace(fpath)

    entry = HistoryEntry(
        board=board_name,
        identity=identity,
        identity_class=cls,
        git_rev=git_rev,
        generated_at=generated_at,
        authoritative=authoritative,
        board_state=state,
        path=str(fpath.relative_to(base)),
    )
    entries = [
        e for e in _load_index_at(board_name, base) if e.get("identity") != identity
    ]
    entries.append(asdict(entry))
    entries.sort(key=lambda e: e.get("generated_at", ""))
    _write_index_at(board_name, base, entries)
    return entry


def latest_authoritative_baseline(
    board_name: str,
    identity_class_key: str,
    *,
    history_dir: Path | None = None,
    exclude_git_rev: str | None = None,
) -> dict[str, Any] | None:
    """The most recent AUTHORITATIVE history board of the same identity-class.

    This is the reference a candidate regresses against. Non-authoritative boards
    never become a baseline (Rule 2). An optional ``exclude_git_rev`` skips the
    candidate's own rev so a re-record does not self-compare."""
    base = history_dir or HISTORY_DIR
    entries = _load_index_at(board_name, base)
    candidates = [
        e
        for e in entries
        if e.get("identity_class") == identity_class_key
        and e.get("authoritative") is True
        and (exclude_git_rev is None or e.get("git_rev") != exclude_git_rev)
    ]
    if not candidates:
        return None
    candidates.sort(key=lambda e: e.get("generated_at", ""))
    best = candidates[-1]
    fpath = base / str(best.get("path"))
    if not fpath.exists():
        return None
    try:
        return json.loads(fpath.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None


# ---------------------------------------------------------------------------
# Board-vs-history regression gate
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Regression:
    cell: str
    severity: str  # "error" (PASS->FAIL flip) | "warn" (drift below baseline)
    baseline_gate: str
    candidate_gate: str
    baseline_warm: float | None
    candidate_warm: float | None
    detail: str


def _flatten_projected_cells(board: Mapping[str, Any]) -> dict[str, dict[str, Any]]:
    """Map ``cell_key -> leaf cell dict`` from a projected board ``table``."""
    out: dict[str, dict[str, Any]] = {}

    def walk(node: Any) -> None:
        if isinstance(node, Mapping):
            if "gate" in node and "benchmark" in node:
                key = f"{node.get('benchmark')} [{node.get('backend')}/{node.get('profile')}]"
                out[key] = dict(node)
            else:
                for child in node.values():
                    walk(child)

    walk(board.get("table", {}))
    return out


def _warm_of(cell: Mapping[str, Any]) -> float | None:
    v = cell.get("warm_speedup")
    return float(v) if isinstance(v, (int, float)) and not isinstance(v, bool) else None


def regressions_vs_baseline(
    candidate: Mapping[str, Any],
    baseline: Mapping[str, Any],
    *,
    drift_warn_fraction: float = DRIFT_WARN_FRACTION,
) -> list[Regression]:
    """Compare a candidate projected board against an authoritative baseline.

    A cell that gated PASS in the baseline and FAILs now is an ERROR regression
    (the previously-green-regressed class). A cell still passing but slower than
    baseline by > ``drift_warn_fraction`` is a WARN drift. A within-fraction
    delta is NOT flagged (the no-false-positive gate, doc 64 §5)."""
    base_cells = _flatten_projected_cells(baseline)
    cand_cells = _flatten_projected_cells(candidate)
    out: list[Regression] = []
    for key, cand in sorted(cand_cells.items()):
        base = base_cells.get(key)
        if base is None:
            continue  # a new cell cannot regress
        base_gate = str(base.get("gate", {}).get("verdict", "?"))
        cand_gate = str(cand.get("gate", {}).get("verdict", "?"))
        base_warm = _warm_of(base)
        cand_warm = _warm_of(cand)
        # ERROR: a PASS cell flipped to FAIL (the gate-blocking regression).
        if base_gate == "PASS" and cand_gate == "FAIL":
            out.append(
                Regression(
                    cell=key,
                    severity="error",
                    baseline_gate=base_gate,
                    candidate_gate=cand_gate,
                    baseline_warm=base_warm,
                    candidate_warm=cand_warm,
                    detail=(
                        f"previously-green cell regressed: "
                        f"warm {_fmt(base_warm)} -> {_fmt(cand_warm)}"
                    ),
                )
            )
            continue
        # WARN: still passing, but materially slower than baseline.
        if (
            base_gate == "PASS"
            and cand_gate == "PASS"
            and base_warm is not None
            and cand_warm is not None
            and cand_warm < base_warm * (1.0 - drift_warn_fraction)
        ):
            pct = (cand_warm / base_warm - 1.0) * 100.0
            out.append(
                Regression(
                    cell=key,
                    severity="warn",
                    baseline_gate=base_gate,
                    candidate_gate=cand_gate,
                    baseline_warm=base_warm,
                    candidate_warm=cand_warm,
                    detail=f"drift (still passing): warm {_fmt(base_warm)} -> "
                    f"{_fmt(cand_warm)} ({pct:+.1f}%)",
                )
            )
    return out


def regression_gate(
    candidate_boards: Mapping[str, Mapping[str, Any]],
    *,
    history_dir: Path | None = None,
) -> dict[str, Any]:
    """Run the board-vs-history regression gate across all candidate boards.

    For each candidate board, find its most-recent authoritative same-class
    baseline in history and collect regressions. Returns a report whose
    ``gate_fails`` is True iff ANY board has an ERROR-severity regression (a
    previously-green cell that regressed)."""
    report: dict[str, Any] = {
        "boards": {},
        "gate_fails": False,
        "errors": [],
        "warnings": [],
    }
    for name, board in candidate_boards.items():
        cls = identity_class(board)
        baseline = latest_authoritative_baseline(
            name,
            cls,
            history_dir=history_dir,
            exclude_git_rev=str(board.get("git_rev", "")),
        )
        if baseline is None:
            report["boards"][name] = {"baseline": None, "regressions": []}
            continue
        regs = regressions_vs_baseline(board, baseline)
        report["boards"][name] = {
            "baseline_git_rev": baseline.get("git_rev"),
            "regressions": [asdict(r) for r in regs],
        }
        for r in regs:
            tagged = f"[{name}] {r.cell}: {r.detail}"
            if r.severity == "error":
                report["errors"].append(tagged)
                report["gate_fails"] = True
            else:
                report["warnings"].append(tagged)
    return report


def _fmt(v: float | None) -> str:
    return "-" if v is None else f"{v:.4f}"


# ---------------------------------------------------------------------------
# CLI — record boards into history and/or run the regression gate
# ---------------------------------------------------------------------------


def _load_boards_from_paths(paths: Sequence[Path]) -> dict[str, dict[str, Any]]:
    """Load one-or-more projected board JSONs keyed by their ``board`` name."""
    out: dict[str, dict[str, Any]] = {}
    for p in paths:
        doc = json.loads(p.read_text(encoding="utf-8"))
        name = str(doc.get("board"))
        if not name:
            raise SystemExit(f"{p}: not a projected board (no 'board' field)")
        out[name] = doc
    return out


def main(argv: list[str] | None = None) -> int:
    import argparse
    import sys

    parser = argparse.ArgumentParser(
        description=(
            "Board history + the board-vs-history regression gate (doc 64 Phase 4). "
            "Records authoritative projected boards and gates a candidate against the "
            "most-recent same-class authoritative baseline (previously-green-regressed)."
        )
    )
    parser.add_argument(
        "boards",
        nargs="+",
        help="projected board JSON path(s) (output of perf_board.py)",
    )
    parser.add_argument(
        "--record",
        action="store_true",
        help="append the board(s) to history + update the index (authoritative boards "
        "become the regression baseline)",
    )
    parser.add_argument(
        "--gate",
        action="store_true",
        help="run the board-vs-history regression gate; exit nonzero on an "
        "ERROR-severity regression (a previously-green cell regressed)",
    )
    parser.add_argument(
        "--history-dir",
        default=None,
        help=f"history root (default: {HISTORY_DIR})",
    )
    parser.add_argument(
        "--no-gate",
        action="store_true",
        help="with --gate, report regressions but always exit 0 (advisory mode)",
    )
    ns = parser.parse_args(argv if argv is not None else sys.argv[1:])

    history_dir = Path(ns.history_dir) if ns.history_dir else None
    try:
        boards = _load_boards_from_paths([Path(p) for p in ns.boards])
    except (OSError, json.JSONDecodeError) as exc:
        print(f"[perf_history] cannot read a board: {exc}", file=sys.stderr)
        return 2

    exit_code = 0

    if ns.gate:
        report = regression_gate(boards, history_dir=history_dir)
        print("\n" + "=" * 100, file=sys.stderr)
        print("BOARD-vs-HISTORY REGRESSION GATE (doc 64 Phase 4)", file=sys.stderr)
        print("=" * 100, file=sys.stderr)
        any_baseline = False
        for name, info in report["boards"].items():
            base_rev = info.get("baseline_git_rev")
            if base_rev is None:
                print(
                    f"  {name:<10} no authoritative same-class baseline in history "
                    "(nothing to regress against)",
                    file=sys.stderr,
                )
                continue
            any_baseline = True
            regs = info.get("regressions", [])
            print(
                f"  {name:<10} baseline={str(base_rev)[:12]}  regressions={len(regs)}",
                file=sys.stderr,
            )
        if report["errors"]:
            print("\n  ERROR regressions (gate-blocking):", file=sys.stderr)
            for e in report["errors"]:
                print(f"    {e}", file=sys.stderr)
        if report["warnings"]:
            print("\n  WARN drift (advisory):", file=sys.stderr)
            for w in report["warnings"]:
                print(f"    {w}", file=sys.stderr)
        if not any_baseline:
            print(
                "  (no baselines yet — record an authoritative run first to arm the gate)",
                file=sys.stderr,
            )
        print("=" * 100, file=sys.stderr)
        if report["gate_fails"] and not ns.no_gate:
            exit_code = 1

    if ns.record:
        for name, board in boards.items():
            entry = record_board(board, history_dir=history_dir)
            tag = "authoritative" if entry.authoritative else "non-authoritative"
            print(
                f"[perf_history] recorded {name} ({tag}, state={entry.board_state}) "
                f"-> {entry.path}",
                file=sys.stderr,
            )

    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
