#!/usr/bin/env python3
"""The perf measurement PLANE: one cell stream, five gated board projections.

doc 64 §3.2 — "a win in one column hides a loss in another" is retired here. The
canonical scoreboard (``perf_scoreboard.py``) measures one stream of cells
(``benchmark x target x backend x profile``). This module is a *pure function* of
that stream: it reduces ``list[cell-dict]`` to FIVE separately-gated board
artifacts (CPython / Backend / Profile / PyPy / Codon), each with its own
``kind`` tag, its own table axes, and — critically — its own gate exit code.

The architectural decision (doc 64 §1 refusal): there is exactly ONE measurement
loop and N *views*. Adding a board is a ``BoardProjection`` value, not a new
measurement script. CPython is the only absolute floor (Performance
Constitution: any benchmark ``warm_speedup < 1.00`` stable+quiescent is RED).
Codon is a ceiling, not a floor (advisory). PyPy gates only on *un-attributed*
losses. Backend gates on cross-backend divergence + each lane's own floor.
Profile holds release-fast/release-output to shipped-perf, dev-fast advisory.

Every board carries the full methodology per cell (the cell dicts already do, by
schema-v3 contract); this module never invents a number — when a comparator lane
did not run (PyPy/Codon host absent), the board records ``ADVISORY`` and an
explicit ``host_absent`` reason, never a faked ratio.
"""

from __future__ import annotations

import datetime as dt
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import Any

from perf_schema import (
    CLASS_RED_STABLE,
    RED_THRESHOLD,
    SCHEMA_VERSION,
    VERDICT_BUILD_FAILED,
    VERDICT_FAIL_COLD_BUDGET,
    VERDICT_FAIL_ENGINE,
    VERDICT_FAIL_STALE,
    VERDICT_RUN_BLOCKED,
    VERDICT_RUN_ERROR,
    VERDICT_UNSTABLE,
    flatten_cells,
)

# --- Per-cell gate verdicts (the projection's atomic decision) --------------

GATE_PASS = "PASS"
GATE_FAIL = "FAIL"
GATE_ADVISORY = "ADVISORY"
GATE_SKIP = "SKIP"

_GATE_VERDICTS = frozenset({GATE_PASS, GATE_FAIL, GATE_ADVISORY, GATE_SKIP})

# Board kinds (durable JSON "kind" tags — one per projection, distinct from the
# canonical "cpython_floor_scoreboard" so a reader can never confuse a projection
# artifact with the source board).
BOARD_KIND = {
    "cpython": "cpython_floor_board",
    "backend": "backend_parity_board",
    "profile": "profile_parity_board",
    "pypy": "pypy_reference_board",
    "codon": "codon_reference_board",
}

# Verdicts that mean "no warm number to gate on" (infra / build / run / blocked /
# stale). These never count as a CPython-floor FAIL by themselves; they are
# surfaced as their own infra tally and (for the CPython board) gate as engine
# failures only via the canonical verdict set below.
_INFRA_VERDICTS = frozenset(
    {
        VERDICT_BUILD_FAILED,
        VERDICT_RUN_ERROR,
        VERDICT_UNSTABLE,
        VERDICT_RUN_BLOCKED,
    }
)

# The hard gate-failing verdicts for the absolute CPython floor (mirrors
# perf_schema.GATE_FAILING_VERDICTS, kept local so the projection predicate is
# self-contained and testable without importing the runner).
_CPYTHON_HARD_FAIL = frozenset(
    {
        VERDICT_FAIL_ENGINE,
        VERDICT_FAIL_COLD_BUDGET,
        VERDICT_BUILD_FAILED,
        VERDICT_RUN_ERROR,
        VERDICT_UNSTABLE,
    }
)


@dataclass(frozen=True)
class GateOutcome:
    """One cell's gate decision inside one board projection."""

    verdict: str  # GATE_PASS / GATE_FAIL / GATE_ADVISORY / GATE_SKIP
    reason: str

    def __post_init__(self) -> None:
        if self.verdict not in _GATE_VERDICTS:
            raise ValueError(f"unknown gate verdict {self.verdict!r}")


def _cell_key(cell: Mapping[str, Any]) -> str:
    return f"{cell.get('benchmark')} [{cell.get('backend')}/{cell.get('profile')}]"


def _warm(cell: Mapping[str, Any]) -> float | None:
    v = cell.get("warm_speedup")
    return float(v) if isinstance(v, (int, float)) and not isinstance(v, bool) else None


def _is_stable(cell: Mapping[str, Any]) -> bool:
    return cell.get("stable") is True


def _is_quiescent(cell: Mapping[str, Any]) -> bool:
    # A warm RED is only authoritative when measured quiescent (Rule 2/3). The
    # cell carries ``measured_quiescent`` when --classify ran; absent that, a
    # board-level authoritative flag governs (passed in via project()).
    return cell.get("measured_quiescent") is True


def _has_cpython_floor(cell: Mapping[str, Any]) -> bool:
    """A cell has a CPython floor unless it is explicitly CPython-incompatible."""
    return cell.get("cpython_incompatible") is not True and cell.get("verdict") != (
        "CPY_INCOMPATIBLE"
    )


# ---------------------------------------------------------------------------
# Gate predicates (one per board)
# ---------------------------------------------------------------------------


def _gate_cpython(cell: Mapping[str, Any], *, board_authoritative: bool) -> GateOutcome:
    """CPython is the ABSOLUTE FLOOR. FAIL iff a stable warm speedup < 1.00, OR a
    hard engine/cold/build/run/unstable verdict. FAIL_STALE on a non-authoritative
    board downgrades to ADVISORY (a noisy/dirty source cannot block, Rule 3)."""
    verdict = str(cell.get("verdict"))
    if not _has_cpython_floor(cell):
        return GateOutcome(GATE_SKIP, "no CPython floor (CPython-incompatible)")
    if verdict == VERDICT_FAIL_STALE:
        return GateOutcome(GATE_ADVISORY, "non-authoritative tree (FAIL_STALE)")
    if verdict == VERDICT_RUN_BLOCKED:
        return GateOutcome(GATE_SKIP, "run-path blocked (build/link only)")
    warm = _warm(cell)
    # A measured warm RED is the canonical CPython-floor violation.
    if warm is not None and warm < RED_THRESHOLD and _is_stable(cell):
        # Quiescence gate: a non-authoritative/non-quiescent board cannot assert
        # an absolute warm RED (it downgrades to advisory). Authoritative boards
        # (quiescent nightly) gate hard.
        if board_authoritative or _is_quiescent(cell):
            return GateOutcome(
                GATE_FAIL,
                f"warm_speedup {warm:.4f} < {RED_THRESHOLD:.2f} (CPython floor)",
            )
        return GateOutcome(
            GATE_ADVISORY,
            f"warm_speedup {warm:.4f} < floor but board non-authoritative",
        )
    if verdict in _CPYTHON_HARD_FAIL:
        if verdict in _INFRA_VERDICTS and not board_authoritative:
            return GateOutcome(GATE_ADVISORY, f"{verdict} on non-authoritative board")
        return GateOutcome(GATE_FAIL, f"{verdict}")
    return GateOutcome(GATE_PASS, "warm at/above CPython floor")


def _gate_profile(cell: Mapping[str, Any], *, board_authoritative: bool) -> GateOutcome:
    """release-fast / release-output are shipped products → hold to the CPython
    floor. dev-fast is compile-latency-optimized → its warm reds are ADVISORY
    (doc 51 profiles table), never a hard FAIL."""
    profile = str(cell.get("profile"))
    base = _gate_cpython(cell, board_authoritative=board_authoritative)
    if profile == "dev-fast" and base.verdict == GATE_FAIL:
        return GateOutcome(GATE_ADVISORY, f"dev-fast advisory: {base.reason}")
    return base


def _gate_pypy(cell: Mapping[str, Any], *, board_authoritative: bool) -> GateOutcome:
    """PyPy is the dynamic reference. Losing is ALLOWED; an *un-attributed* loss
    is the failure (doc 64 §3.2): ``pypy_ratio < 1.00`` with no named
    ``pypy_advantage_class`` FAILs. A loss WITH a named mechanism PASSes
    (the missing fact is recorded). No PyPy lane → SKIP (host absent)."""
    ratio = cell.get("pypy_ratio")
    if not isinstance(ratio, (int, float)) or isinstance(ratio, bool):
        return GateOutcome(GATE_SKIP, "no PyPy lane (host absent)")
    ratio = float(ratio)
    if ratio >= RED_THRESHOLD:
        return GateOutcome(GATE_PASS, f"pypy_ratio {ratio:.4f} >= floor (molt >= PyPy)")
    advantage = cell.get("pypy_advantage_class")
    if isinstance(advantage, str) and advantage:
        return GateOutcome(
            GATE_PASS,
            f"loss to PyPy ({ratio:.4f}) attributed to {advantage}",
        )
    return GateOutcome(
        GATE_FAIL,
        f"un-attributed loss to PyPy ({ratio:.4f}): no pypy_advantage_class",
    )


def _gate_codon(cell: Mapping[str, Any], *, board_authoritative: bool) -> GateOutcome:
    """Codon is the AOT *ceiling*, not a floor (CLAUDE.md). It NEVER hard-FAILs on
    "you lost to Codon"; it ADVISES (approach/match/exceed). Only equivalent-tagged
    cells are compared at all (non-equivalent semantics are excluded by the
    cell_filter). No Codon lane → SKIP (host absent)."""
    ratio = cell.get("codon_ratio")
    if not isinstance(ratio, (int, float)) or isinstance(ratio, bool):
        return GateOutcome(GATE_SKIP, "no Codon lane (host absent)")
    ratio = float(ratio)
    if ratio >= RED_THRESHOLD:
        return GateOutcome(GATE_ADVISORY, f"exceeds/matches Codon ({ratio:.4f})")
    return GateOutcome(GATE_ADVISORY, f"approaching Codon ({ratio:.4f})")


# ---------------------------------------------------------------------------
# Cell filters (which cells a board owns)
# ---------------------------------------------------------------------------


def _filter_all(cell: Mapping[str, Any]) -> bool:
    return True


def _filter_pypy_eligible(cell: Mapping[str, Any]) -> bool:
    """PyPy board owns dynamic-class cells (when tagged) OR any cell that actually
    has a PyPy ratio. ``reference_class`` may be absent (Phase 6 suite tags not
    yet applied) — in that case the presence of a pypy_ratio is the membership
    signal, so the board lights up the moment a PyPy lane runs."""
    if cell.get("pypy_ratio") is not None:
        return True
    return cell.get("reference_class") == "dynamic"


def _filter_codon_eligible(cell: Mapping[str, Any]) -> bool:
    """Codon board owns only semantically-equivalent cells (doc 64: non-equivalent
    is excluded from win/loss by construction). ``codon_semantics`` may be absent
    (suite tags pending) — then a present codon_ratio is the membership signal,
    UNLESS the cell is explicitly tagged non_equivalent."""
    if cell.get("codon_semantics") == "non_equivalent":
        return False
    if cell.get("codon_ratio") is not None:
        return True
    return cell.get("codon_semantics") == "equivalent"


# ---------------------------------------------------------------------------
# BoardProjection
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class BoardProjection:
    """One gated view over the canonical cell stream (doc 64 §3.2)."""

    name: str
    kind: str
    cell_filter: Callable[[Mapping[str, Any]], bool]
    group_by: tuple[str, ...]
    gate_predicate: Callable[..., GateOutcome]
    # When True, an empty board (no owned cells) is ADVISORY (host absent) not a
    # hard error — for the PyPy/Codon comparator boards that only exist when the
    # comparator binary is installed.
    advisory_when_empty: bool = False
    description: str = ""

    def select(self, cells: Sequence[Mapping[str, Any]]) -> list[Mapping[str, Any]]:
        return [c for c in cells if self.cell_filter(c)]


# The five canonical projections (doc 64 §3.2). Each is a single value; the plane
# is `for proj in PROJECTIONS: proj.project(...)`.
PROJECTIONS: tuple[BoardProjection, ...] = (
    BoardProjection(
        name="cpython",
        kind=BOARD_KIND["cpython"],
        cell_filter=_filter_all,
        group_by=("benchmark", "backend", "profile"),
        gate_predicate=_gate_cpython,
        description="absolute floor: any stable warm_speedup < 1.00 is RED",
    ),
    BoardProjection(
        name="backend",
        kind=BOARD_KIND["backend"],
        cell_filter=_filter_all,
        group_by=("backend", "benchmark", "profile"),
        gate_predicate=_gate_cpython,  # each lane held to its own CPython floor
        description="native/llvm/wasm/luau each its own table; a native win never "
        "excuses a wasm regression (cross-backend divergence FAILs)",
    ),
    BoardProjection(
        name="profile",
        kind=BOARD_KIND["profile"],
        cell_filter=_filter_all,
        group_by=("profile", "benchmark", "backend"),
        gate_predicate=_gate_profile,
        description="dev-fast/release-fast/release-output each its own table; "
        "release-* held to shipped-perf, dev-fast advisory",
    ),
    BoardProjection(
        name="pypy",
        kind=BOARD_KIND["pypy"],
        cell_filter=_filter_pypy_eligible,
        group_by=("benchmark", "backend"),
        gate_predicate=_gate_pypy,
        advisory_when_empty=True,
        description="dynamic reference; an un-attributed loss (no pypy_advantage_"
        "class) FAILs; host absent -> ADVISORY",
    ),
    BoardProjection(
        name="codon",
        kind=BOARD_KIND["codon"],
        cell_filter=_filter_codon_eligible,
        group_by=("benchmark", "backend"),
        gate_predicate=_gate_codon,
        advisory_when_empty=True,
        description="AOT ceiling (advisory only); equivalent-tagged cells only; "
        "host absent -> ADVISORY",
    ),
)

PROJECTIONS_BY_NAME: dict[str, BoardProjection] = {p.name: p for p in PROJECTIONS}


# ---------------------------------------------------------------------------
# Projection -> board document
# ---------------------------------------------------------------------------


def _grouped_table(
    cells: Sequence[Mapping[str, Any]],
    outcomes: Mapping[str, GateOutcome],
    group_by: tuple[str, ...],
) -> dict[str, Any]:
    """Nest cells by the board's group_by axes; each leaf carries the full
    methodology row (the cell) plus this board's gate outcome."""
    table: dict[str, Any] = {}
    for cell in cells:
        key = _cell_key(cell)
        outcome = outcomes[key]
        node = table
        for axis in group_by[:-1]:
            node = node.setdefault(str(cell.get(axis)), {})
        leaf_key = str(cell.get(group_by[-1]))
        node[leaf_key] = {
            "benchmark": cell.get("benchmark"),
            "target": cell.get("target"),
            "backend": cell.get("backend"),
            "profile": cell.get("profile"),
            "warm_speedup": cell.get("warm_speedup"),
            "cold_speedup": cell.get("cold_speedup"),
            "startup_tax_ms": cell.get("startup_tax_ms"),
            "binary_size_kib": cell.get("binary_size_kib"),
            "molt_peak_rss_mib": cell.get("molt_peak_rss_mib"),
            "compile_time_s": cell.get("compile_time_s"),
            "pypy_ratio": cell.get("pypy_ratio"),
            "codon_ratio": cell.get("codon_ratio"),
            "verdict": cell.get("verdict"),
            "classification": cell.get("classification"),
            "stable": cell.get("stable"),
            "reference_class": cell.get("reference_class"),
            "codon_semantics": cell.get("codon_semantics"),
            "pypy_advantage_class": cell.get("pypy_advantage_class"),
            "fact_class": cell.get("fact_class"),
            "suspected_missing_fact": cell.get("suspected_missing_fact"),
            "attribution_confidence": cell.get("attribution_confidence"),
            "log_artifact": cell.get("log_artifact"),
            "gate": {"verdict": outcome.verdict, "reason": outcome.reason},
        }
    return table


def _cross_backend_divergences(
    cells: Sequence[Mapping[str, Any]],
) -> list[dict[str, Any]]:
    """A *cross-backend* divergence (doc 64 Backend board): a (benchmark, profile)
    where one backend lane has a stable warm RED that another lane does NOT. This
    is the "a native win never excuses a wasm regression" invariant — surfaced
    explicitly so a reader sees the asymmetry, not just per-lane reds."""
    by_bp: dict[tuple[str, str], dict[str, float | None]] = {}
    for cell in cells:
        bench = str(cell.get("benchmark"))
        profile = str(cell.get("profile"))
        backend = str(cell.get("backend"))
        warm = _warm(cell) if _is_stable(cell) else None
        by_bp.setdefault((bench, profile), {})[backend] = warm
    out: list[dict[str, Any]] = []
    for (bench, profile), lanes in sorted(by_bp.items()):
        red = {b for b, w in lanes.items() if w is not None and w < RED_THRESHOLD}
        green = {b for b, w in lanes.items() if w is not None and w >= RED_THRESHOLD}
        if red and green:
            out.append(
                {
                    "benchmark": bench,
                    "profile": profile,
                    "red_backends": sorted(red),
                    "green_backends": sorted(green),
                    "warm_by_backend": {b: lanes[b] for b in sorted(lanes)},
                }
            )
    return out


def project(
    projection: BoardProjection,
    cells: Sequence[Mapping[str, Any]],
    *,
    source_meta: Mapping[str, Any],
    board_authoritative: bool,
) -> dict[str, Any]:
    """Reduce the canonical cell stream to ONE gated board document.

    ``source_meta`` carries the identity the board inherits from its source
    (git_rev, provenance, host, generated_at). ``board_authoritative`` is the
    source board's authoritative flag — a non-authoritative source can only emit
    ADVISORY warm verdicts (Rule 3)."""
    owned = projection.select(cells)
    outcomes: dict[str, GateOutcome] = {}
    for cell in owned:
        outcomes[_cell_key(cell)] = projection.gate_predicate(
            cell, board_authoritative=board_authoritative
        )

    tally = {GATE_PASS: 0, GATE_FAIL: 0, GATE_ADVISORY: 0, GATE_SKIP: 0}
    fails: list[dict[str, str]] = []
    advisories: list[dict[str, str]] = []
    for cell in owned:
        outcome = outcomes[_cell_key(cell)]
        tally[outcome.verdict] += 1
        if outcome.verdict == GATE_FAIL:
            fails.append({"cell": _cell_key(cell), "reason": outcome.reason})
        elif outcome.verdict == GATE_ADVISORY:
            advisories.append({"cell": _cell_key(cell), "reason": outcome.reason})

    # Board-level state: empty comparator board is ADVISORY (host absent), not a
    # hard error. Any FAIL cell makes the board FAIL. Otherwise PASS.
    if not owned:
        board_state = GATE_ADVISORY if projection.advisory_when_empty else GATE_SKIP
        board_reason = (
            "no owned cells (comparator host absent)"
            if projection.advisory_when_empty
            else "no owned cells"
        )
    elif fails:
        board_state = GATE_FAIL
        board_reason = f"{len(fails)} cell(s) failed the {projection.name} gate"
    else:
        board_state = GATE_PASS
        board_reason = (
            f"all {len(owned)} owned cell(s) within the {projection.name} gate"
        )

    summary: dict[str, Any] = {
        "board_state": board_state,
        "board_reason": board_reason,
        "cells_owned": len(owned),
        "cells_pass": tally[GATE_PASS],
        "cells_fail": tally[GATE_FAIL],
        "cells_advisory": tally[GATE_ADVISORY],
        "cells_skip": tally[GATE_SKIP],
        "fails": fails,
        "advisories": advisories,
        "red_stable_cells": sorted(
            _cell_key(c) for c in owned if c.get("classification") == CLASS_RED_STABLE
        ),
    }
    if projection.name == "backend":
        summary["cross_backend_divergences"] = _cross_backend_divergences(owned)
        if summary["cross_backend_divergences"] and board_state != GATE_FAIL:
            # A cross-backend divergence is itself a Backend-board FAIL even if
            # each lane individually passes its own floor (the asymmetry is the
            # bug). This is the doc-51 §3 "a native win never excuses a wasm
            # regression" invariant enforced by exit code.
            summary["board_state"] = GATE_FAIL
            summary["board_reason"] = (
                f"{len(summary['cross_backend_divergences'])} cross-backend "
                "divergence(s): a lane regressed where another lane is green"
            )

    return {
        "schema_version": SCHEMA_VERSION,
        "kind": projection.kind,
        "board": projection.name,
        "description": projection.description,
        "generated_at": source_meta.get("generated_at")
        or dt.datetime.now(dt.timezone.utc).isoformat(),
        "git_rev": source_meta.get("git_rev"),
        "source_kind": source_meta.get("source_kind", "cpython_floor_scoreboard"),
        "source_authoritative": board_authoritative,
        "red_threshold": RED_THRESHOLD,
        "group_by": list(projection.group_by),
        "provenance": dict(source_meta.get("provenance", {})),
        "host": dict(source_meta.get("host", {})),
        "summary": summary,
        "table": _grouped_table(owned, outcomes, projection.group_by),
    }


def project_all(
    source_doc: Mapping[str, Any],
) -> dict[str, dict[str, Any]]:
    """Project a canonical ``cpython_floor_scoreboard`` document into ALL five
    boards. Returns ``{board_name: board_doc}``."""
    cells = flatten_cells(source_doc)
    provenance = source_doc.get("provenance", {})
    board_authoritative = bool(
        provenance.get("authoritative", True)
        if isinstance(provenance, Mapping)
        else True
    )
    source_meta = {
        "generated_at": source_doc.get("generated_at"),
        "git_rev": source_doc.get("git_rev"),
        "source_kind": source_doc.get("kind", "cpython_floor_scoreboard"),
        "provenance": provenance,
        "host": source_doc.get("host", {}),
    }
    return {
        proj.name: project(
            proj,
            cells,
            source_meta=source_meta,
            board_authoritative=board_authoritative,
        )
        for proj in PROJECTIONS
    }


def board_gate_exit_code(boards: Mapping[str, Mapping[str, Any]]) -> int:
    """The PLANE gate: nonzero iff ANY board's ``board_state`` is FAIL.

    This is the "five boards, five exit codes, one merged plane verdict" of doc
    64 §3.2 — a native win cannot hide a wasm regression because the Backend
    board has its own FAIL that this OR-reduces in."""
    for board in boards.values():
        if board.get("summary", {}).get("board_state") == GATE_FAIL:
            return 1
    return 0


def plane_summary_line(boards: Mapping[str, Mapping[str, Any]]) -> str:
    """One-line human summary across the plane (the CLAUDE.md landing-report seed)."""
    parts: list[str] = []
    for name in ("cpython", "backend", "profile", "pypy", "codon"):
        board = boards.get(name)
        if board is None:
            continue
        s = board.get("summary", {})
        state = s.get("board_state")
        parts.append(
            f"{name}={state}({s.get('cells_fail', 0)}F/{s.get('cells_owned', 0)})"
        )
    return "  ".join(parts)


# ---------------------------------------------------------------------------
# CLI — project a source scoreboard into the five boards (a pure consumer)
# ---------------------------------------------------------------------------


def _print_plane(boards: Mapping[str, Mapping[str, Any]]) -> None:
    import sys

    print("\n" + "=" * 100, file=sys.stderr)
    print(
        "PERF MEASUREMENT PLANE — five gated projections (doc 64 §3.2)", file=sys.stderr
    )
    print("=" * 100, file=sys.stderr)
    hdr = f"{'BOARD':<10}{'KIND':<26}{'STATE':<10}{'OWN':>4}{'PASS':>6}{'FAIL':>6}{'ADV':>5}{'SKIP':>6}"
    print(hdr, file=sys.stderr)
    print("-" * 100, file=sys.stderr)
    for name in ("cpython", "backend", "profile", "pypy", "codon"):
        board = boards.get(name)
        if board is None:
            continue
        s = board.get("summary", {})
        print(
            f"{name:<10}{board.get('kind', '?'):<26}{s.get('board_state', '?'):<10}"
            f"{s.get('cells_owned', 0):>4}{s.get('cells_pass', 0):>6}"
            f"{s.get('cells_fail', 0):>6}{s.get('cells_advisory', 0):>5}"
            f"{s.get('cells_skip', 0):>6}",
            file=sys.stderr,
        )
    print("-" * 100, file=sys.stderr)
    for name in ("cpython", "backend", "profile", "pypy", "codon"):
        board = boards.get(name)
        if board is None:
            continue
        fails = board.get("summary", {}).get("fails", [])
        if fails:
            print(f"\n{name.upper()} BOARD FAILS ({len(fails)}):", file=sys.stderr)
            for f in fails:
                print(f"    {f['cell']}  ->  {f['reason']}", file=sys.stderr)
        divs = board.get("summary", {}).get("cross_backend_divergences")
        if divs:
            print(
                f"\n{name.upper()} CROSS-BACKEND DIVERGENCES ({len(divs)}):",
                file=sys.stderr,
            )
            for d in divs:
                print(
                    f"    {d['benchmark']} [{d['profile']}]  RED={d['red_backends']}  "
                    f"GREEN={d['green_backends']}",
                    file=sys.stderr,
                )
    print("=" * 100, file=sys.stderr)


def main(argv: list[str] | None = None) -> int:
    import argparse
    import json
    import sys
    from pathlib import Path

    parser = argparse.ArgumentParser(
        description=(
            "Project a canonical perf scoreboard into the five gated boards "
            "(CPython / Backend / Profile / PyPy / Codon). A pure consumer of the "
            "measurement core — runs no benchmarks."
        )
    )
    parser.add_argument(
        "source",
        help="path to a canonical cpython_floor_scoreboard JSON (the source cell stream)",
    )
    parser.add_argument(
        "--out-dir",
        default=None,
        help="directory to write the five board artifacts (default: alongside source, "
        "named <board>_<gitrev>.json)",
    )
    parser.add_argument(
        "--board",
        action="append",
        choices=list(PROJECTIONS_BY_NAME),
        default=None,
        help="only project the named board(s) (repeatable; default: all five)",
    )
    parser.add_argument(
        "--no-gate",
        action="store_true",
        help="always exit 0 (project + write only; do not fail on a board FAIL)",
    )
    parser.add_argument(
        "--quiet",
        action="store_true",
        help="suppress the plane table (still writes artifacts + exit code)",
    )
    ns = parser.parse_args(argv if argv is not None else sys.argv[1:])

    src_path = Path(ns.source)
    try:
        source_doc = json.loads(src_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        print(f"[perf_board] cannot read source {src_path}: {exc}", file=sys.stderr)
        return 2

    boards = project_all(source_doc)
    if ns.board:
        boards = {k: v for k, v in boards.items() if k in set(ns.board)}

    out_dir = Path(ns.out_dir) if ns.out_dir else src_path.parent
    out_dir.mkdir(parents=True, exist_ok=True)
    git_rev = str(source_doc.get("git_rev", "unknown"))[:12]
    for name, board in boards.items():
        out_path = out_dir / f"{name}_{git_rev}.json"
        tmp = out_path.with_suffix(".tmp")
        tmp.write_text(json.dumps(board, indent=2) + "\n", encoding="utf-8")
        tmp.replace(out_path)
        print(f"[perf_board] {name} board -> {out_path}", file=sys.stderr)

    if not ns.quiet:
        _print_plane(boards)
    print("[perf_board] plane: " + plane_summary_line(boards), file=sys.stderr)

    if ns.no_gate:
        return 0
    return board_gate_exit_code(boards)


if __name__ == "__main__":
    raise SystemExit(main())
