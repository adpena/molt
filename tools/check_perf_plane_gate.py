#!/usr/bin/env python3
"""Fail-closed dry-run: the perf-plane gate MUST reject a CPython-red regression.

doc 64 Phase 3 / §5 — the merge gate is only trustworthy if it actually FAILS on
the thing it claims to catch. This is the falsifiable self-proof of the plane:
synthesize an authoritative all-green baseline, record it to a throwaway history,
then synthesize a candidate where one previously-green cell flips below the
CPython floor, and assert BOTH:

  (1) ``perf_board.board_gate_exit_code`` is nonzero for a board carrying the red
      cell (the ABSOLUTE-floor gate), and
  (2) ``perf_history.regression_gate`` is nonzero for the previously-green->red
      flip (the REGRESSION gate).

If either gate would pass a CPython-red, the perf plane certifies nothing and CI
must fail HERE (the proxy-measurement meta-bug class: a gate that cannot fail is
a vacuous gate). This checker is wired into ci_gate Tier 1 and reuses the SAME
projection + history authority the real CI workflow uses — it is not a parallel
re-implementation.
"""

from __future__ import annotations

import copy
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
TOOLS = REPO / "tools"
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

import perf_board as pb  # noqa: E402
import perf_history as ph  # noqa: E402
import perf_schema as schema  # noqa: E402


def _green_cell() -> dict[str, object]:
    return {
        "benchmark": "tests/benchmarks/bench_fib.py",
        "target": "native",
        "backend": "native",
        "profile": "release-fast",
        "build_ok": True,
        "run_blocked": False,
        "molt_ok": True,
        "cpython_ok": True,
        "cold_molt_s": 0.10,
        "cold_cpython_s": 0.20,
        "warm_molt_s": 0.10,
        "warm_cpython_s": 0.20,
        "warm_speedup": 2.0,
        "cold_speedup": 2.0,
        "startup_tax_ms": 5.0,
        "verdict": schema.VERDICT_GREEN,
        "binary_size_kib": 512.0,
        "molt_peak_rss_mib": 18.0,
        "compile_time_s": 0.4,
        "stable": True,
        "measured_quiescent": True,
        "pypy_ratio": None,
        "codon_ratio": None,
        "codon_equivalent": None,
        "cpython_peak_rss_mib": 15.0,
        "output_parity": True,
        "log_artifact": "bench/scoreboard/logs/fib.log",
        "classification": schema.CLASS_GREEN,
    }


def _source(cell: dict[str, object], *, git_rev: str) -> dict:
    return {
        "schema_version": schema.SCHEMA_VERSION,
        "kind": "cpython_floor_scoreboard",
        "generated_at": f"2026-06-24T00:00:0{git_rev[0]}+00:00",
        "git_rev": git_rev,
        "provenance": {"authoritative": True, "benchmark_tool_sha": "tool" + "0" * 36},
        "host": {
            "platform": "linux",
            "arch": "x86_64",
            "pointer_bits": 64,
            "cpython_baseline": "3.14.5",
        },
        "scoreboard": {
            cell["benchmark"]: {
                cell["target"]: {cell["backend"]: {cell["profile"]: cell}}
            }
        },
    }


def check() -> list[str]:
    """Return a list of problems; empty == the plane gate correctly fails closed."""
    problems: list[str] = []

    baseline = _source(_green_cell(), git_rev="a" * 40)
    baseline_boards = pb.project_all(baseline)

    # The absolute floor must PASS an all-green board (no false positive).
    if pb.board_gate_exit_code(baseline_boards) != 0:
        problems.append(
            "perf_board.board_gate_exit_code FAILED an all-green baseline "
            "(false positive: the floor gate blocks a healthy board)"
        )

    # Candidate: the previously-green cell flips below the CPython floor.
    red = copy.deepcopy(_green_cell())
    red["warm_speedup"] = 0.75
    red["cold_speedup"] = 0.75
    red["verdict"] = schema.VERDICT_FAIL_ENGINE
    cand = _source(red, git_rev="d" * 40)
    cand_boards = pb.project_all(cand)

    # (1) Absolute-floor gate must reject the red.
    if pb.board_gate_exit_code(cand_boards) == 0:
        problems.append(
            "perf_board.board_gate_exit_code PASSED a CPython-red board "
            "(the absolute-floor gate is vacuous — it cannot fail on a warm RED)"
        )
    if cand_boards["cpython"]["summary"]["board_state"] != pb.GATE_FAIL:
        problems.append(
            "CPython board did not FAIL on a stable warm_speedup < 1.00 "
            f"(got {cand_boards['cpython']['summary']['board_state']})"
        )

    # (2) Regression gate must reject the previously-green -> red flip.
    with tempfile.TemporaryDirectory() as td:
        hist = Path(td) / "history"
        for board in baseline_boards.values():
            ph.record_board(board, history_dir=hist)
        report = ph.regression_gate(cand_boards, history_dir=hist)
        if not report["gate_fails"]:
            problems.append(
                "perf_history.regression_gate PASSED a previously-green CPython-red "
                "regression (the regression gate is vacuous)"
            )
        if not any("[cpython]" in e for e in report["errors"]):
            problems.append(
                "regression gate did not attribute the red to the CPython board"
            )

        # No-false-positive: an identical re-run of the baseline must NOT regress.
        rerun = _source(_green_cell(), git_rev="e" * 40)
        rerun_boards = pb.project_all(rerun)
        clean = ph.regression_gate(rerun_boards, history_dir=hist)
        if clean["gate_fails"]:
            problems.append(
                "regression gate FALSE-POSITIVE: an unchanged re-run was flagged "
                f"as a regression ({clean['errors']})"
            )

    return problems


def main() -> int:
    problems = check()
    if problems:
        print(
            "[perf-plane-gate] FAIL — the plane gate does not fail closed:",
            file=sys.stderr,
        )
        for p in problems:
            print(f"    - {p}", file=sys.stderr)
        return 1
    print(
        "[perf-plane-gate] OK: the absolute-floor gate AND the board-vs-history "
        "regression gate both reject a synthetic CPython-red; an unchanged re-run "
        "is not flagged (no false positive).",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
