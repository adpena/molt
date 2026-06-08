"""Unit tests for the two-dimensional perf-scoreboard classification + gate.

These exercise the verdict logic, provenance, and gate exit code with SYNTHETIC
cells — no molt rebuild required (the council's classification-logic test does
not need real benchmark runs). The measurement path itself is covered by the
tool's ``--self-test`` (one real bench_fib build).
"""

from __future__ import annotations

import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
for _p in (REPO_ROOT / "tools", REPO_ROOT / "src"):
    if str(_p) not in sys.path:
        sys.path.insert(0, str(_p))

import perf_scoreboard as ps  # noqa: E402


def _cell(
    *,
    benchmark: str = "tests/benchmarks/bench_fib.py",
    backend: str = "native",
    profile: str = "release-fast",
    build_ok: bool = True,
    molt_ok: bool = True,
    cpython_ok: bool = True,
    stable: bool = True,
    warm_molt_s: float | None = None,
    warm_cpython_s: float | None = None,
    cold_molt_s: float | None = None,
    cold_cpython_s: float | None = None,
    run_blocked: bool = False,
) -> ps.Cell:
    c = ps.Cell(benchmark=benchmark, target="native", backend=backend, profile=profile)
    c.build_ok = build_ok
    c.molt_ok = molt_ok
    c.cpython_ok = cpython_ok
    c.stable = stable
    c.warm_molt_s = warm_molt_s
    c.warm_cpython_s = warm_cpython_s
    c.cold_molt_s = cold_molt_s
    c.cold_cpython_s = cold_cpython_s
    c.run_blocked = run_blocked
    return c


# --- Verdict classification -------------------------------------------------


def test_green_warm_and_cold_fast() -> None:
    # molt 2x faster warm AND cold; tiny tax within budget.
    c = _cell(
        warm_molt_s=0.10,
        warm_cpython_s=0.20,
        cold_molt_s=0.12,
        cold_cpython_s=0.25,
    )
    c.finalize(budget_ms=100.0, authoritative=True)
    assert c.verdict == ps.VERDICT_GREEN
    assert c.warm_speedup == 2.0
    assert c.cold_speedup is not None and c.cold_speedup > 1.0
    assert c.startup_tax_ms == 20.0  # (0.12 - 0.10) * 1000
    assert c.red is False


def test_fail_engine_when_warm_at_or_below_floor() -> None:
    # Warm steady-state SLOWER than CPython -> FAIL_ENGINE (release blocker).
    c = _cell(
        warm_molt_s=0.20,
        warm_cpython_s=0.12,  # cpython faster warm
        cold_molt_s=0.30,
        cold_cpython_s=0.30,
    )
    c.finalize(budget_ms=1000.0, authoritative=True)
    assert c.verdict == ps.VERDICT_FAIL_ENGINE
    assert c.warm_speedup is not None and c.warm_speedup < 1.0
    assert c.red is True
    assert c.suspected_missing_fact  # a triage hint is attached


def test_fail_engine_at_exactly_one() -> None:
    # warm_speedup == 1.00 is NOT a win (<=, per ruling A).
    c = _cell(
        warm_molt_s=0.10,
        warm_cpython_s=0.10,
        cold_molt_s=0.10,
        cold_cpython_s=0.10,
    )
    c.finalize(budget_ms=1000.0, authoritative=True)
    assert c.verdict == ps.VERDICT_FAIL_ENGINE


def test_warn_cold_floor_when_warm_fast_but_cold_slow_within_budget() -> None:
    # Warm 2x faster, but cold path loses to CPython purely on a fixed tax that
    # is WITHIN budget -> WARN_COLD_FLOOR (not a hard red).
    c = _cell(
        warm_molt_s=0.010,
        warm_cpython_s=0.020,
        cold_molt_s=0.060,  # tax = 50ms
        cold_cpython_s=0.040,  # cpython cold faster -> cold_speedup < 1
    )
    c.finalize(budget_ms=100.0, authoritative=True)
    assert c.verdict == ps.VERDICT_WARN_COLD_FLOOR
    assert c.warm_speedup is not None and c.warm_speedup > 1.0
    assert c.cold_speedup is not None and c.cold_speedup < 1.0
    assert c.startup_tax_ms == 50.0
    assert c.red is False  # warns, does not hard-fail


def test_fail_cold_budget_when_tax_exceeds_budget() -> None:
    # Warm fast, but the fixed startup tax exceeds the budget -> FAIL_COLD_BUDGET.
    c = _cell(
        warm_molt_s=0.010,
        warm_cpython_s=0.020,
        cold_molt_s=0.260,  # tax = 250ms
        cold_cpython_s=0.040,
    )
    c.finalize(budget_ms=100.0, authoritative=True)
    assert c.verdict == ps.VERDICT_FAIL_COLD_BUDGET
    assert c.startup_tax_ms == 250.0
    assert c.cold_budget_ms == 100.0
    assert c.red is True
    assert c.suspected_startup_component


def test_no_budget_means_cold_budget_cannot_fire() -> None:
    # Without a recorded budget, an over-tax cold cell does NOT FAIL_COLD_BUDGET
    # (we never invent a budget); it warns or is green on the cold axis.
    c = _cell(
        warm_molt_s=0.010,
        warm_cpython_s=0.020,
        cold_molt_s=0.500,  # huge tax, but no budget
        cold_cpython_s=0.040,
    )
    c.finalize(budget_ms=None, authoritative=True)
    assert c.verdict == ps.VERDICT_WARN_COLD_FLOOR  # cold<1 but warm>1
    assert c.red is False


def test_fail_stale_overrides_everything() -> None:
    # A green-looking cell on a non-authoritative tree is FAIL_STALE.
    c = _cell(
        warm_molt_s=0.10,
        warm_cpython_s=0.20,
        cold_molt_s=0.12,
        cold_cpython_s=0.25,
    )
    c.finalize(budget_ms=100.0, authoritative=False)
    assert c.verdict == ps.VERDICT_FAIL_STALE
    assert c.red is True


def test_unstable_is_gated() -> None:
    c = _cell(
        stable=False,
        warm_molt_s=0.10,
        warm_cpython_s=0.20,
        cold_molt_s=0.12,
        cold_cpython_s=0.25,
    )
    c.finalize(budget_ms=100.0, authoritative=True)
    assert c.verdict == ps.VERDICT_UNSTABLE
    assert c.red is True


def test_build_failed_and_run_error_and_blocked_and_incompat() -> None:
    bf = _cell(build_ok=False)
    bf.finalize(authoritative=True)
    assert bf.verdict == ps.VERDICT_BUILD_FAILED and bf.red is True

    re = _cell(molt_ok=False)
    re.finalize(authoritative=True)
    assert re.verdict == ps.VERDICT_RUN_ERROR and re.red is True

    rb = _cell(run_blocked=True)
    rb.finalize(authoritative=True)
    assert rb.verdict == ps.VERDICT_RUN_BLOCKED and rb.red is False

    ci = _cell(cpython_ok=False)
    ci.finalize(authoritative=True)
    assert ci.verdict == ps.VERDICT_CPY_INCOMPAT and ci.red is False


# --- Gate exit code ---------------------------------------------------------


def _board(cells: list[ps.Cell], provenance: dict | None = None) -> dict:
    return ps.build_scoreboard_doc(
        cells,
        benchmarks_run=[c.benchmark for c in cells],
        benchmarks_deferred=[],
        cpython_version="3.14",
        samples=5,
        warmup=2,
        provenance=provenance or {"authoritative": True},
    )


def test_gate_fails_on_engine_red() -> None:
    engine_red = _cell(
        warm_molt_s=0.20, warm_cpython_s=0.10, cold_molt_s=0.2, cold_cpython_s=0.2
    )
    engine_red.finalize(budget_ms=1000.0, authoritative=True)
    doc = _board([engine_red])
    assert ps._gate_exit_code(doc, no_gate=False) == 1


def test_gate_passes_on_all_green() -> None:
    g = _cell(
        warm_molt_s=0.10, warm_cpython_s=0.20, cold_molt_s=0.11, cold_cpython_s=0.25
    )
    g.finalize(budget_ms=1000.0, authoritative=True)
    doc = _board([g])
    assert ps._gate_exit_code(doc, no_gate=False) == 0


def test_gate_warn_cold_floor_does_not_fail_unless_strict() -> None:
    w = _cell(
        warm_molt_s=0.010,
        warm_cpython_s=0.020,
        cold_molt_s=0.060,
        cold_cpython_s=0.040,
    )
    w.finalize(budget_ms=100.0, authoritative=True)
    doc = _board([w])
    assert w.verdict == ps.VERDICT_WARN_COLD_FLOOR
    assert ps._gate_exit_code(doc, no_gate=False) == 0
    assert ps._gate_exit_code(doc, no_gate=False, strict_cold=True) == 1


def test_gate_fail_stale_unless_allow_nonauthoritative() -> None:
    s = _cell(
        warm_molt_s=0.10, warm_cpython_s=0.20, cold_molt_s=0.11, cold_cpython_s=0.25
    )
    s.finalize(budget_ms=1000.0, authoritative=False)
    doc = _board([s], provenance={"authoritative": False})
    assert ps._gate_exit_code(doc, no_gate=False) == 1
    assert ps._gate_exit_code(doc, no_gate=False, allow_nonauthoritative=True) == 0


def test_gate_no_gate_always_zero() -> None:
    engine_red = _cell(
        warm_molt_s=0.20, warm_cpython_s=0.10, cold_molt_s=0.2, cold_cpython_s=0.2
    )
    engine_red.finalize(budget_ms=1000.0, authoritative=True)
    doc = _board([engine_red])
    assert ps._gate_exit_code(doc, no_gate=True) == 0


# --- Schema + provenance ----------------------------------------------------


def test_board_carries_provenance_and_passes_schema() -> None:
    g = _cell(
        warm_molt_s=0.10, warm_cpython_s=0.20, cold_molt_s=0.11, cold_cpython_s=0.25
    )
    g.finalize(budget_ms=1000.0, authoritative=True)
    prov = {
        "origin_sha": "a" * 40,
        "local_head_sha": "a" * 40,
        "merge_base_sha": "a" * 40,
        "dirty_tree": False,
        "benchmark_tool_sha": "b" * 40,
        "backend_binary_identity": {"native/release-fast": "x|1|2"},
        "stdlib_cache_key": "deadbeef",
        "authoritative": True,
    }
    doc = _board([g], provenance=prov)
    problems = ps._validate_schema(doc)
    assert problems == [], f"schema problems: {problems}"
    assert doc["provenance"]["origin_sha"] == "a" * 40
    assert doc["summary"]["cells_green"] == 1
    assert doc["summary"]["gate_fails"] is False


def test_verdict_breakdown_routes_warm_vs_cold() -> None:
    warm = _cell(
        benchmark="tests/benchmarks/bench_etl_orders.py",
        warm_molt_s=0.20,
        warm_cpython_s=0.10,
        cold_molt_s=0.3,
        cold_cpython_s=0.3,
    )
    warm.finalize(budget_ms=1000.0, authoritative=True)
    cold = _cell(
        benchmark="tests/benchmarks/bench_import_time.py",
        warm_molt_s=0.010,
        warm_cpython_s=0.020,
        cold_molt_s=0.260,
        cold_cpython_s=0.040,
    )
    cold.finalize(budget_ms=100.0, authoritative=True)
    doc = _board([warm, cold])
    vb = doc["summary"]["verdict_breakdown"]
    assert any("etl_orders" in k for k in vb["FAIL_ENGINE"])
    assert any("import_time" in k for k in vb["FAIL_COLD_BUDGET"])
    # Never blended.
    assert not any("etl_orders" in k for k in vb["FAIL_COLD_BUDGET"])


def test_authoritative_reason_lists_causes() -> None:
    assert "origin/main" in ps._authoritative_reason(True, False, False)
    assert "dirty" in ps._authoritative_reason(False, True, False)
    assert "perf_scoreboard.py" in ps._authoritative_reason(False, False, True)
    assert ps._authoritative_reason(False, False, False) == (
        "tree == origin/main, clean, tool unmodified"
    )


def test_codon_equivalence_allowlist_is_conservative() -> None:
    # A non-kernel benchmark is NOT on the Codon allowlist.
    assert "tests/benchmarks/bench_etl_orders.py" not in ps.CODON_EQUIVALENT_BENCHMARKS
    # A numeric kernel IS.
    assert "tests/benchmarks/bench_fib.py" in ps.CODON_EQUIVALENT_BENCHMARKS
