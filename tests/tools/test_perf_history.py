"""Contract tests for board history + the board-vs-history regression gate (doc 64 Phase 4).

Proves the second Performance-Constitution triage axis is GATEABLE: a
previously-green cell that regressed FAILs the gate; a within-CI noise delta does
NOT (the no-false-positive gate); a non-authoritative board never becomes a
baseline (Rule 2). SYNTHETIC boards only — no molt rebuild.
"""

from __future__ import annotations

import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import perf_board as pb  # noqa: E402
import perf_history as ph  # noqa: E402
import perf_schema as schema  # noqa: E402


def _cell(**overrides: object) -> dict[str, object]:
    cell: dict[str, object] = {
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
    cell.update(overrides)
    return cell


def _source_doc(
    cells: list[dict[str, object]],
    *,
    authoritative: bool = True,
    git_rev: str = "a" * 40,
) -> dict:
    nested: dict = {}
    for c in cells:
        (
            nested.setdefault(c["benchmark"], {})
            .setdefault(c["target"], {})
            .setdefault(c["backend"], {})
        )[c["profile"]] = c
    return {
        "schema_version": schema.SCHEMA_VERSION,
        "kind": "cpython_floor_scoreboard",
        "generated_at": f"2026-06-24T00:00:0{git_rev[0]}+00:00",
        "git_rev": git_rev,
        "provenance": {
            "authoritative": authoritative,
            "benchmark_tool_sha": "tool" + "0" * 36,
        },
        "host": {
            "platform": "linux",
            "arch": "x86_64",
            "pointer_bits": 64,
            "cpython_baseline": "3.14.5",
        },
        "scoreboard": nested,
    }


# --- Board identity ---------------------------------------------------------


def test_identity_class_stable_across_revs_same_tool_suite_host() -> None:
    a = pb.project_all(_source_doc([_cell()], git_rev="a" * 40))["cpython"]
    b = pb.project_all(_source_doc([_cell()], git_rev="b" * 40))["cpython"]
    # Same tool + suite + host => same comparability class (so they CAN be
    # regression-compared) even though git_rev differs.
    assert ph.identity_class(a) == ph.identity_class(b)
    # But the full board_identity differs (content-addressed by rev).
    assert ph.board_identity(a) != ph.board_identity(b)


def test_identity_class_differs_across_host() -> None:
    a = pb.project_all(_source_doc([_cell()]))["cpython"]
    b_doc = _source_doc([_cell()])
    b_doc["host"]["platform"] = "win32"
    b = pb.project_all(b_doc)["cpython"]
    assert ph.identity_class(a) != ph.identity_class(b)


# --- Record + baseline retrieval --------------------------------------------


def test_only_authoritative_boards_become_baselines(tmp_path: Path) -> None:
    hist = tmp_path / "history"
    non_auth = pb.project_all(_source_doc([_cell()], authoritative=False))["cpython"]
    ph.record_board(non_auth, history_dir=hist)
    # A non-authoritative board is recorded but NEVER selected as a baseline.
    baseline = ph.latest_authoritative_baseline(
        "cpython", ph.identity_class(non_auth), history_dir=hist
    )
    assert baseline is None

    auth = pb.project_all(_source_doc([_cell()], authoritative=True, git_rev="c" * 40))[
        "cpython"
    ]
    ph.record_board(auth, history_dir=hist)
    baseline = ph.latest_authoritative_baseline(
        "cpython", ph.identity_class(auth), history_dir=hist
    )
    assert baseline is not None
    assert baseline["git_rev"] == "c" * 40


# --- The regression gate (the headline deliverable) -------------------------


def test_regression_gate_fails_on_previously_green_cpython_red(tmp_path: Path) -> None:
    hist = tmp_path / "history"
    # Baseline: an authoritative all-green board.
    baseline_boards = pb.project_all(
        _source_doc([_cell()], authoritative=True, git_rev="a" * 40)
    )
    for board in baseline_boards.values():
        ph.record_board(board, history_dir=hist)

    # Candidate: a different rev, the same cell flipped CPython-RED.
    cand_doc = _source_doc(
        [_cell(warm_speedup=0.75, verdict=schema.VERDICT_FAIL_ENGINE)],
        authoritative=True,
        git_rev="d" * 40,
    )
    cand_boards = pb.project_all(cand_doc)

    report = ph.regression_gate(cand_boards, history_dir=hist)
    assert report["gate_fails"] is True
    assert any("previously-green" in e for e in report["errors"])
    assert any("[cpython]" in e for e in report["errors"])


def test_regression_gate_silent_on_within_threshold_noise(tmp_path: Path) -> None:
    hist = tmp_path / "history"
    baseline_boards = pb.project_all(
        _source_doc([_cell(warm_speedup=2.0)], authoritative=True, git_rev="a" * 40)
    )
    for board in baseline_boards.values():
        ph.record_board(board, history_dir=hist)

    # 2% slower, still well above the floor: NOT a regression (no false positive).
    cand_boards = pb.project_all(
        _source_doc([_cell(warm_speedup=1.96)], authoritative=True, git_rev="d" * 40)
    )
    report = ph.regression_gate(cand_boards, history_dir=hist)
    assert report["gate_fails"] is False
    assert report["errors"] == []
    # 2% < DRIFT_WARN_FRACTION (5%) => not even a warn.
    assert report["warnings"] == []


def test_regression_gate_warns_on_material_but_passing_drift(tmp_path: Path) -> None:
    hist = tmp_path / "history"
    baseline_boards = pb.project_all(
        _source_doc([_cell(warm_speedup=2.0)], authoritative=True, git_rev="a" * 40)
    )
    for board in baseline_boards.values():
        ph.record_board(board, history_dir=hist)

    # 20% slower but STILL passing the floor: WARN drift, not a gate-blocking error.
    cand_boards = pb.project_all(
        _source_doc([_cell(warm_speedup=1.60)], authoritative=True, git_rev="d" * 40)
    )
    report = ph.regression_gate(cand_boards, history_dir=hist)
    assert report["gate_fails"] is False  # still passing -> does not block
    assert any("drift" in w for w in report["warnings"])


def test_regression_gate_no_baseline_is_not_a_failure(tmp_path: Path) -> None:
    hist = tmp_path / "history"  # empty
    cand_boards = pb.project_all(
        _source_doc([_cell(warm_speedup=0.5, verdict=schema.VERDICT_FAIL_ENGINE)])
    )
    report = ph.regression_gate(cand_boards, history_dir=hist)
    # No baseline to regress against => the regression gate is silent (the
    # ABSOLUTE-floor gate, perf_board, is what catches a brand-new red).
    assert report["gate_fails"] is False


def test_record_is_idempotent_by_identity(tmp_path: Path) -> None:
    hist = tmp_path / "history"
    board = pb.project_all(_source_doc([_cell()], git_rev="a" * 40))["cpython"]
    ph.record_board(board, history_dir=hist)
    ph.record_board(board, history_dir=hist)  # same identity -> replaces, no dup
    index = ph._load_index_at("cpython", hist)
    assert len(index) == 1


def test_full_cli_chain_project_record_gate(tmp_path: Path) -> None:
    """Exercise the CLI surface CI uses: write a source board, project it, record
    the authoritative baseline, then gate a regressed candidate -> exit 1."""
    import json

    src = tmp_path / "cpython_source.json"
    src.write_text(
        json.dumps(_source_doc([_cell()], authoritative=True, git_rev="a" * 40)),
        encoding="utf-8",
    )
    board_dir = tmp_path / "boards"
    hist = tmp_path / "history"

    # project + record the baseline (no-gate: all green anyway)
    rc = pb.main([str(src), "--out-dir", str(board_dir), "--no-gate", "--quiet"])
    assert rc == 0
    rc = ph.main(
        [
            str(board_dir / "cpython_aaaaaaaaaaaa.json"),
            "--record",
            "--history-dir",
            str(hist),
        ]
    )
    assert rc == 0

    # candidate regressed
    cand_src = tmp_path / "cpython_cand.json"
    cand_src.write_text(
        json.dumps(
            _source_doc(
                [_cell(warm_speedup=0.7, verdict=schema.VERDICT_FAIL_ENGINE)],
                authoritative=True,
                git_rev="d" * 40,
            )
        ),
        encoding="utf-8",
    )
    cand_dir = tmp_path / "cand_boards"
    rc = pb.main([str(cand_src), "--out-dir", str(cand_dir), "--no-gate", "--quiet"])
    assert rc == 0
    rc = ph.main(
        [
            str(cand_dir / "cpython_dddddddddddd.json"),
            "--gate",
            "--history-dir",
            str(hist),
        ]
    )
    assert rc == 1  # the regression gate fails CI on the previously-green red
