"""Contract tests for the five-board perf PLANE projections (doc 64 §3.2).

These prove the load-bearing invariant the plane exists to enforce: a win in one
column cannot hide a loss in another, because each board is a SEPARATELY-GATED
projection of one canonical cell stream. SYNTHETIC cells only — no molt rebuild
(the projection is a pure function of the cell stream).
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import perf_board as pb  # noqa: E402
import perf_schema as schema  # noqa: E402


def _cell(**overrides: object) -> dict[str, object]:
    """A GREEN-by-default raw cell dict (the shape flatten_cells yields)."""
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


def _source_doc(cells: list[dict[str, object]], *, authoritative: bool = True) -> dict:
    """A minimal canonical cpython_floor_scoreboard doc wrapping raw cells."""
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
        "generated_at": "2026-06-24T00:00:00+00:00",
        "git_rev": "a" * 40,
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


# --- Projection structure ---------------------------------------------------


def test_project_all_emits_five_distinct_boards() -> None:
    boards = pb.project_all(_source_doc([_cell()]))
    assert set(boards) == {"cpython", "backend", "profile", "pypy", "codon"}
    kinds = {b["kind"] for b in boards.values()}
    assert len(kinds) == 5  # each board has its OWN kind tag (not one shared board)
    assert boards["cpython"]["kind"] == "cpython_floor_board"


def test_all_green_plane_passes() -> None:
    boards = pb.project_all(_source_doc([_cell()]))
    assert boards["cpython"]["summary"]["board_state"] == pb.GATE_PASS
    assert pb.board_gate_exit_code(boards) == 0


# --- The CPython absolute floor (any warm < 1.00 is RED) --------------------


def test_cpython_floor_fails_on_stable_warm_red() -> None:
    red = _cell(
        benchmark="tests/benchmarks/bench_slow.py",
        warm_speedup=0.80,
        cold_speedup=0.80,
        verdict=schema.VERDICT_FAIL_ENGINE,
    )
    boards = pb.project_all(_source_doc([_cell(), red]))
    cpy = boards["cpython"]["summary"]
    assert cpy["board_state"] == pb.GATE_FAIL
    assert cpy["cells_fail"] == 1
    assert any("bench_slow" in f["cell"] for f in cpy["fails"])
    assert pb.board_gate_exit_code(boards) == 1


def test_non_authoritative_board_downgrades_warm_red_to_advisory() -> None:
    # A warm RED on a non-quiescent, non-authoritative board cannot HARD-fail
    # (Rule 3) — it is advisory, never a blocking RED.
    red = _cell(
        warm_speedup=0.80,
        verdict=schema.VERDICT_FAIL_ENGINE,
        measured_quiescent=False,
    )
    boards = pb.project_all(_source_doc([red], authoritative=False))
    assert boards["cpython"]["summary"]["board_state"] != pb.GATE_FAIL
    assert boards["cpython"]["summary"]["cells_advisory"] == 1


def test_quiescent_warm_red_gates_even_on_dirty_tree() -> None:
    # Quiescence (not tree-cleanliness) is the authority for a warm RED: a
    # quiescent measurement on a dirty tree is still a real CPython-floor red.
    red = _cell(
        warm_speedup=0.80, verdict=schema.VERDICT_FAIL_ENGINE, measured_quiescent=True
    )
    boards = pb.project_all(_source_doc([red], authoritative=False))
    assert boards["cpython"]["summary"]["board_state"] == pb.GATE_FAIL


# --- The Backend board: a native win cannot hide a wasm regression ----------


def test_backend_board_flags_cross_backend_divergence() -> None:
    native_green = _cell(backend="native", warm_speedup=2.0)
    wasm_red = _cell(
        backend="wasm", warm_speedup=0.70, verdict=schema.VERDICT_FAIL_ENGINE
    )
    boards = pb.project_all(_source_doc([native_green, wasm_red]))
    backend = boards["backend"]["summary"]
    assert backend["board_state"] == pb.GATE_FAIL
    divs = backend["cross_backend_divergences"]
    assert len(divs) == 1
    assert divs[0]["red_backends"] == ["wasm"]
    assert divs[0]["green_backends"] == ["native"]
    # And the CPython board ALSO fails (the wasm lane is below its own floor),
    # but the asymmetry is surfaced explicitly on the Backend board.
    assert pb.board_gate_exit_code(boards) == 1


# --- The Profile board: dev-fast warm reds are advisory ---------------------


def test_profile_board_dev_fast_red_is_advisory_not_fail() -> None:
    dev_red = _cell(
        profile="dev-fast", warm_speedup=0.80, verdict=schema.VERDICT_FAIL_ENGINE
    )
    boards = pb.project_all(_source_doc([dev_red]))
    prof = boards["profile"]["summary"]
    assert prof["board_state"] != pb.GATE_FAIL  # dev-fast warm red does not block
    assert prof["cells_advisory"] == 1


def test_profile_board_release_fast_red_fails() -> None:
    rel_red = _cell(
        profile="release-fast", warm_speedup=0.80, verdict=schema.VERDICT_FAIL_ENGINE
    )
    boards = pb.project_all(_source_doc([rel_red]))
    assert boards["profile"]["summary"]["board_state"] == pb.GATE_FAIL


# --- The PyPy board: un-attributed loss FAILs, attributed loss PASSes -------


def test_pypy_board_advisory_when_host_absent() -> None:
    # No pypy_ratio anywhere -> the comparator host is absent -> ADVISORY, never
    # a fake number and never a hard fail.
    boards = pb.project_all(_source_doc([_cell()]))
    assert boards["pypy"]["summary"]["board_state"] == pb.GATE_ADVISORY
    assert boards["pypy"]["summary"]["cells_owned"] == 0


def test_pypy_board_fails_on_unattributed_loss() -> None:
    loss = _cell(pypy_ratio=0.85)  # molt slower than PyPy, no mechanism named
    boards = pb.project_all(_source_doc([loss]))
    pypy = boards["pypy"]["summary"]
    assert pypy["board_state"] == pb.GATE_FAIL
    assert any("un-attributed" in f["reason"] for f in pypy["fails"])


def test_pypy_board_passes_on_attributed_loss() -> None:
    loss = _cell(pypy_ratio=0.85, pypy_advantage_class="loop_specialization")
    boards = pb.project_all(_source_doc([loss]))
    assert boards["pypy"]["summary"]["board_state"] == pb.GATE_PASS


def test_pypy_board_passes_when_molt_wins() -> None:
    win = _cell(pypy_ratio=1.30)
    boards = pb.project_all(_source_doc([win]))
    assert boards["pypy"]["summary"]["board_state"] == pb.GATE_PASS


# --- The Codon board: ceiling, never a hard floor ---------------------------


def test_codon_board_never_hard_fails_on_loss() -> None:
    # Losing to Codon is ADVISORY (a ceiling), never a hard FAIL.
    loss = _cell(codon_ratio=0.50, codon_semantics="equivalent")
    boards = pb.project_all(_source_doc([loss]))
    cod = boards["codon"]["summary"]
    assert cod["board_state"] != pb.GATE_FAIL
    assert cod["cells_owned"] == 1
    assert cod["cells_advisory"] == 1


def test_codon_board_excludes_non_equivalent_cells() -> None:
    non_equiv = _cell(codon_ratio=0.50, codon_semantics="non_equivalent")
    boards = pb.project_all(_source_doc([non_equiv]))
    # Excluded from the win/loss board by construction.
    assert boards["codon"]["summary"]["cells_owned"] == 0


# --- Real committed board round-trips through the plane ----------------------


def test_committed_quiet_native_projects_cleanly() -> None:
    doc = json.loads(
        (REPO_ROOT / "bench" / "scoreboard" / "quiet_native.json").read_text(
            encoding="utf-8"
        )
    )
    boards = pb.project_all(doc)
    assert set(boards) == {"cpython", "backend", "profile", "pypy", "codon"}
    # Every owned cell has a gate verdict; every board carries a methodology row.
    for board in boards.values():
        for cell in _walk_leaves(board["table"]):
            assert "gate" in cell and cell["gate"]["verdict"] in pb._GATE_VERDICTS
            # Methodology fields the constitution requires per row.
            for fld in ("warm_speedup", "binary_size_kib", "compile_time_s"):
                assert fld in cell


def _walk_leaves(node: object) -> list[dict]:
    out: list[dict] = []
    if isinstance(node, dict):
        if "gate" in node and "benchmark" in node:
            out.append(node)
        else:
            for child in node.values():
                out.extend(_walk_leaves(child))
    return out
