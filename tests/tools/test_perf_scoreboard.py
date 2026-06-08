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


def test_backend_binary_resolver_probes_target_roots(monkeypatch, tmp_path) -> None:
    # When CARGO_TARGET_DIR has the binary, the resolver finds it; otherwise it
    # degrades to None (never crashes). We point every probed root at empty
    # tmp dirs, then materialize the binary under CARGO_TARGET_DIR.
    cargo = tmp_path / "cargo"
    (cargo / "release-fast").mkdir(parents=True)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(cargo))
    # Repoint the other two probed roots away from the real repo so the test is
    # hermetic (the real target/ may or may not have a binary).
    monkeypatch.setattr(ps, "REPO_ROOT", tmp_path / "norepo")
    assert ps._resolve_backend_binary_path(ps.NATIVE_CRANELIFT, "release-fast") is None
    binpath = cargo / "release-fast" / "molt-backend"
    binpath.write_bytes(b"\x7fELF-stub")
    found = ps._resolve_backend_binary_path(ps.NATIVE_CRANELIFT, "release-fast")
    assert found == binpath


def test_gather_provenance_authoritative_when_clean(monkeypatch) -> None:
    # With identical origin/local SHAs, a clean tree, and an unmodified tool,
    # the board is authoritative.
    monkeypatch.setattr(ps, "_git_output", lambda args: _fake_git(args))
    monkeypatch.setattr(
        ps,
        "_benchmark_tool_identity",
        lambda: {
            "ondisk_blob_sha": "b",
            "modified_vs_head": "false",
            "last_commit_sha": "c",
        },
    )
    monkeypatch.setattr(ps, "_stdlib_cache_key_signal", lambda: "deadbeef")
    prov = ps.gather_provenance(None)
    assert prov["authoritative"] is True
    assert prov["origin_sha"] == prov["local_head_sha"]
    assert prov["dirty_tree"] is False


def _fake_git(args: list[str]) -> str | None:
    if args[:2] == ["rev-parse", "HEAD"]:
        return "a" * 40
    if args == ["rev-parse", "origin/main"]:
        return "a" * 40
    if args[:1] == ["merge-base"]:
        return "a" * 40
    if args == ["status", "--porcelain"]:
        return None  # clean
    return None


# --- Robust stability (CPython outlier tolerance) ---------------------------


def _phase(samples: list[float]) -> ps.PhaseStats:
    return ps.PhaseStats.from_runs(
        [ps.RunOutcome(True, s, 8.0, "ok", 0) for s in samples]
    )


def test_robust_stable_molt_unstable_is_never_stable() -> None:
    # molt is the artifact under test; if molt is unstable the cell is unstable
    # regardless of CPython.
    molt = _phase([0.05, 0.20, 0.05, 0.20, 0.05])  # high CV
    cpy = _phase([0.40, 0.41, 0.40, 0.41, 0.40])
    assert molt.stable is False
    assert ps._robust_cell_stable(molt, cpy) is False


def test_robust_stable_both_stable() -> None:
    molt = _phase([0.05, 0.051, 0.05, 0.052, 0.05])
    cpy = _phase([0.40, 0.41, 0.40, 0.41, 0.40])
    assert ps._robust_cell_stable(molt, cpy) is True


def test_robust_stable_cpython_outlier_but_verdict_robust() -> None:
    # The class_hierarchy case: molt rock-stable 8x faster, CPython has one GC
    # spike (cv > 0.20) but even its FASTEST sample keeps molt > 1.0 -> stable.
    molt = _phase([0.054, 0.053, 0.053, 0.050, 0.053])  # cv ~0.03
    cpy = _phase([0.417, 0.427, 0.424, 0.415, 0.637])  # cv ~0.23 (outlier)
    assert molt.stable is True
    assert cpy.stable is False
    # CPython's min 0.415 / molt median 0.053 ~= 7.8x > 1.0; max likewise.
    assert ps._robust_cell_stable(molt, cpy) is True


def test_robust_stable_cpython_straddles_floor_is_unstable() -> None:
    # molt ~= CPython (near 1.0) and CPython's distribution genuinely straddles
    # molt's median even after trimming one outlier each side: the verdict could
    # flip, so the cell is correctly UNSTABLE. cpy sorted=[.085,.092,.100,.108,.140]
    # trimmed bounds [.092,.108] -> 0.92x and 1.08x straddle 1.0.
    molt = _phase([0.100, 0.101, 0.100, 0.102, 0.100])
    cpy = _phase([0.085, 0.092, 0.100, 0.108, 0.140])
    assert ps._robust_cell_stable(molt, cpy) is False


def test_robust_stable_tolerates_single_fast_cpython_outlier() -> None:
    # The json_roundtrip case: molt rock-stable, CPython median 1.7x slower but
    # ONE anomalously-fast sample (0.019) equals molt's median. The raw-min/max
    # rule would call it UNSTABLE (min/median == 1.0); trimming that one outlier
    # leaves the 2nd-fastest clearly > molt -> STABLE (a real molt win).
    molt = _phase([0.018, 0.019, 0.019, 0.019, 0.019])  # median 0.019
    cpy = _phase([0.019, 0.033, 0.036, 0.045, 0.032])  # one fast outlier 0.019
    assert molt.stable is True
    assert cpy.stable is False
    assert ps._robust_cell_stable(molt, cpy) is True


# ===========================================================================
# #69 measurement-hygiene additions: quiescence guard, repeat CI, 5-state
# classification, and cycle attribution — ALL exercised with SYNTHETIC inputs
# (no molt build, no live machine probe). The council requires the
# classification + quiescence LOGIC be unit-tested independently of a real run.
# ===========================================================================


def _warm_cell(
    warm_speedup: float,
    *,
    stable: bool = True,
    repeat_stability: str | None = None,
    repeat_ci: tuple[float, float] | None = None,
    repeat_passes: int | None = None,
) -> ps.Cell:
    """A finalized-shape cell with a chosen warm_speedup for classify tests.

    We set warm_speedup directly (the classifier reads it) plus the repeat-CI
    fields so we can drive every branch of classify_cell without a real run.
    """
    c = ps.Cell(
        benchmark="tests/benchmarks/bench_x.py",
        target="native",
        backend="native",
        profile="release-fast",
    )
    c.build_ok = c.molt_ok = c.cpython_ok = True
    c.stable = stable
    c.warm_speedup = warm_speedup
    c.verdict = ps.VERDICT_FAIL_ENGINE if warm_speedup <= 1.0 else ps.VERDICT_GREEN
    if repeat_stability is not None:
        c.repeat_stability = repeat_stability
    if repeat_ci is not None:
        c.repeat_ci_lo, c.repeat_ci_hi = repeat_ci
    if repeat_passes is not None:
        c.repeat_passes = repeat_passes
    return c


# --- Repeat-pass CI ---------------------------------------------------------


def test_warm_ci_single_pass_has_no_interval() -> None:
    # One sample -> median only; no fabricated CI (council: never invent a tight
    # CI from a single pass).
    median, var, lo, hi = ps._warm_speedup_ci([0.66])
    assert median == 0.66
    assert var is None and lo is None and hi is None


def test_warm_ci_tight_red_cluster_is_below_one() -> None:
    # Five tightly-clustered sub-1.0 passes -> CI sits entirely below 1.00.
    median, var, lo, hi = ps._warm_speedup_ci([0.60, 0.61, 0.59, 0.60, 0.62])
    assert 0.59 <= median <= 0.62
    assert hi is not None and hi < 1.00
    assert ps._repeat_stability(lo, hi) == "STABLE_BELOW"


def test_warm_ci_straddling_cluster_crosses_one() -> None:
    # Wide spread bracketing 1.0 -> the CI straddles -> a TIE, not a target.
    median, var, lo, hi = ps._warm_speedup_ci([0.80, 1.30, 0.70, 1.20, 0.95])
    assert lo is not None and hi is not None
    assert lo < 1.00 < hi
    assert ps._repeat_stability(lo, hi) == "STRADDLES"


def test_warm_ci_tight_green_cluster_is_above_one() -> None:
    median, var, lo, hi = ps._warm_speedup_ci([2.00, 2.05, 1.98, 2.02, 2.01])
    assert lo is not None and lo > 1.00
    assert ps._repeat_stability(lo, hi) == "STABLE_ABOVE"


def test_repeat_stability_unconfirmed_when_no_ci() -> None:
    assert ps._repeat_stability(None, None) == "UNCONFIRMED"


def test_warm_ci_empty_is_all_none() -> None:
    assert ps._warm_speedup_ci([]) == (None, None, None, None)


# --- 5-state classification: RED_STABLE / RED_NOISY -------------------------


def test_classify_red_stable_requires_quiescent_stable_and_ci_below() -> None:
    # The TRUE warm-red: quiescent + stable + repeat CI entirely below 1.0.
    c = _warm_cell(
        0.60, stable=True, repeat_stability="STABLE_BELOW",
        repeat_ci=(0.58, 0.62), repeat_passes=5,
    )
    cls, reason = ps.classify_cell(c, quiescent=True)
    assert cls == ps.CLASS_RED_STABLE
    assert "TRUE compiler target" in reason


def test_classify_red_noisy_when_not_quiescent() -> None:
    # Same numbers but the machine was NOT quiet -> demoted to RED_NOISY, and the
    # reason NAMES contamination (this is exactly the "0.66 under load" artifact).
    c = _warm_cell(
        0.66, stable=True, repeat_stability="STABLE_BELOW",
        repeat_ci=(0.62, 0.70), repeat_passes=5,
    )
    cls, reason = ps.classify_cell(c, quiescent=False)
    assert cls == ps.CLASS_RED_NOISY
    assert "NOT quiescent" in reason


def test_classify_red_noisy_when_unstable() -> None:
    c = _warm_cell(
        0.60, stable=False, repeat_stability="STABLE_BELOW",
        repeat_ci=(0.58, 0.62), repeat_passes=5,
    )
    cls, reason = ps.classify_cell(c, quiescent=True)
    assert cls == ps.CLASS_RED_NOISY
    assert "unstable" in reason


def test_classify_red_noisy_single_pass_no_ci_is_not_yet_target() -> None:
    # A sub-1.0 cell on a quiet stable machine but with NO repeat CI is NOT yet
    # RED_STABLE — it is RED_NOISY pending --repeat confirmation (Rule: a target
    # needs a confidence interval, not a point estimate).
    c = _warm_cell(0.60, stable=True)  # no repeat fields
    cls, reason = ps.classify_cell(c, quiescent=True)
    assert cls == ps.CLASS_RED_NOISY
    assert "no repeat CI" in reason


def test_classify_ci_governs_over_point_estimate() -> None:
    # A noisy single-pass point estimate of 0.98 (< 1.0) but a repeat CI that
    # clears ABOVE 1.0 is governed by the CI, not the point estimate: the cell is
    # a real GREEN. This proves the repeat CI — not a lone point sample — decides
    # the side of the floor (the council's whole reason for --repeat).
    c = _warm_cell(
        0.98, stable=True, repeat_stability="STABLE_ABOVE",
        repeat_ci=(1.01, 1.10), repeat_passes=5,
    )
    cls, _ = ps.classify_cell(c, quiescent=True)
    assert cls == ps.CLASS_GREEN


# --- 5-state classification: TIE -------------------------------------------


def test_classify_tie_when_ci_straddles() -> None:
    c = _warm_cell(
        0.95, stable=True, repeat_stability="STRADDLES",
        repeat_ci=(0.85, 1.15), repeat_passes=5,
    )
    cls, reason = ps.classify_cell(c, quiescent=True)
    assert cls == ps.CLASS_TIE
    assert "straddles 1.00" in reason


def test_classify_tie_when_warm_exactly_one_single_pass() -> None:
    c = _warm_cell(1.00, stable=True)
    cls, reason = ps.classify_cell(c, quiescent=True)
    assert cls == ps.CLASS_TIE
    assert "statistically CPython" in reason


# --- 5-state classification: GREEN -----------------------------------------


def test_classify_green_stable_quiescent() -> None:
    c = _warm_cell(
        2.50, stable=True, repeat_stability="STABLE_ABOVE",
        repeat_ci=(2.40, 2.60), repeat_passes=5,
    )
    cls, _ = ps.classify_cell(c, quiescent=True)
    assert cls == ps.CLASS_GREEN


def test_classify_green_single_pass_quiescent_stable() -> None:
    # A clear warm win on a quiet stable machine is GREEN even without a repeat
    # CI (a win does not need the same statistical bar a target-red does, but it
    # still must be quiescent + stable).
    c = _warm_cell(3.00, stable=True)
    cls, _ = ps.classify_cell(c, quiescent=True)
    assert cls == ps.CLASS_GREEN


def test_classify_green_survives_load_contamination_is_conservative() -> None:
    # ASYMMETRY OF CONTAMINATION: load can only make molt look SLOWER, never
    # faster. So a clear warm WIN measured under contamination is still a real
    # GREEN (the quiet number would be even better) — NOT a RED_NOISY. Only
    # instability demotes a green. (A 10x cell must not be mislabeled RED just
    # because an idle daemon was running.)
    c = _warm_cell(3.00, stable=True)
    cls, reason = ps.classify_cell(c, quiescent=False)
    assert cls == ps.CLASS_GREEN
    assert "conservative" in reason


def test_classify_green_above_demoted_to_noisy_only_when_unstable() -> None:
    # A point-above cell that is UNSTABLE (volatile samples) is not a confirmed
    # win -> RED_NOISY (green unconfirmed). Instability, not load, is what blocks
    # a green.
    c = _warm_cell(3.00, stable=False)
    cls, reason = ps.classify_cell(c, quiescent=True)
    assert cls == ps.CLASS_RED_NOISY
    assert "green unconfirmed" in reason


# --- 5-state classification: DIMENSIONAL_WIN -------------------------------


def test_dimensional_win_on_tie_with_material_rss_drop() -> None:
    # A warm TIE (==1.00) that nonetheless dropped RSS materially vs a baseline is
    # a DIMENSIONAL_WIN per Rule 4 (landed without a warm flip, better elsewhere).
    c = _warm_cell(1.00, stable=True)
    c.molt_peak_rss_mib = 80.0
    baseline = {"molt_peak_rss_mib": 120.0}  # 33% RSS reduction
    cls, reason = ps.classify_cell(c, quiescent=True, baseline_cell=baseline)
    assert cls == ps.CLASS_DIMENSIONAL_WIN
    assert "RSS" in reason


def test_dimensional_win_on_straddle_with_binary_shrink() -> None:
    c = _warm_cell(
        0.97, stable=True, repeat_stability="STRADDLES",
        repeat_ci=(0.90, 1.05), repeat_passes=5,
    )
    c.binary_size_kib = 1800.0
    baseline = {"binary_size_kib": 2400.0}  # 25% smaller
    cls, reason = ps.classify_cell(c, quiescent=True, baseline_cell=baseline)
    assert cls == ps.CLASS_DIMENSIONAL_WIN
    assert "binary" in reason


def test_dimensional_win_requires_material_delta() -> None:
    # A sub-threshold (2%) improvement is NOT a dimensional win -> stays TIE.
    c = _warm_cell(1.00, stable=True)
    c.molt_peak_rss_mib = 98.0
    baseline = {"molt_peak_rss_mib": 100.0}  # only 2% < 5% gate
    cls, _ = ps.classify_cell(c, quiescent=True, baseline_cell=baseline)
    assert cls == ps.CLASS_TIE


def test_dimensional_win_needs_a_baseline() -> None:
    c = _warm_cell(1.00, stable=True)
    c.molt_peak_rss_mib = 50.0
    cls, _ = ps.classify_cell(c, quiescent=True, baseline_cell=None)
    assert cls == ps.CLASS_TIE


def test_dimensional_improvement_higher_is_better_for_cold() -> None:
    c = _warm_cell(1.00, stable=True)
    c.cold_speedup = 1.50
    baseline = {"cold_speedup": 1.00}  # +50% cold improvement
    reason = ps._dimensional_improvement(c, baseline)
    assert reason is not None and "cold" in reason


# --- 5-state classification: INFRA passthrough -----------------------------


def test_classify_infra_passthrough_for_build_failed() -> None:
    c = _warm_cell(0.0, stable=False)
    c.verdict = ps.VERDICT_BUILD_FAILED
    cls, _ = ps.classify_cell(c, quiescent=True)
    assert cls == ps.CLASS_INFRA


def test_classify_infra_when_no_warm_number() -> None:
    c = _warm_cell(1.0, stable=True)
    c.warm_speedup = None
    c.verdict = ps.VERDICT_GREEN
    cls, _ = ps.classify_cell(c, quiescent=True)
    assert cls == ps.CLASS_INFRA


# --- apply_classification across a board ------------------------------------


def test_apply_classification_sets_state_on_every_cell() -> None:
    red = _warm_cell(0.60, stable=True, repeat_stability="STABLE_BELOW",
                     repeat_ci=(0.58, 0.62), repeat_passes=5)
    green = _warm_cell(2.0, stable=True, repeat_stability="STABLE_ABOVE",
                       repeat_ci=(1.9, 2.1), repeat_passes=5)
    green.benchmark = "tests/benchmarks/bench_y.py"
    cells = [red, green]
    ps.apply_classification(cells, quiescent=True)
    assert red.classification == ps.CLASS_RED_STABLE
    assert green.classification == ps.CLASS_GREEN
    assert red.measured_quiescent is True and green.measured_quiescent is True


def test_apply_classification_contaminated_demotes_all_reds() -> None:
    red = _warm_cell(0.60, stable=True, repeat_stability="STABLE_BELOW",
                     repeat_ci=(0.58, 0.62), repeat_passes=5)
    ps.apply_classification([red], quiescent=False)
    assert red.classification == ps.CLASS_RED_NOISY
    assert red.measured_quiescent is False


def test_apply_classification_asymmetry_red_noisy_but_green_survives() -> None:
    # On a NON-quiescent board, a warm RED is demoted to RED_NOISY (load can
    # manufacture a false red) but a clear warm GREEN STAYS GREEN_STABLE (load
    # can only have made the win look smaller — it is a conservative green). This
    # is the board-level twin of the asymmetry rule, exercised the way
    # --rebuild-summary re-applies it from a stored non-quiescent board.
    red = _warm_cell(0.60, stable=True, repeat_stability="STABLE_BELOW",
                     repeat_ci=(0.58, 0.62), repeat_passes=5)
    green = _warm_cell(10.5, stable=True, repeat_stability="STABLE_ABOVE",
                       repeat_ci=(9.9, 10.9), repeat_passes=3)
    green.benchmark = "tests/benchmarks/bench_sum.py"
    ps.apply_classification([red, green], quiescent=False)
    assert red.classification == ps.CLASS_RED_NOISY
    assert green.classification == ps.CLASS_GREEN
    assert green.measured_quiescent is False  # recorded, but not gating the green


# --- Quiescence gather (synthetic; monkeypatched probes) --------------------


def test_gather_quiescence_quiet_when_idle(monkeypatch) -> None:
    monkeypatch.setattr(ps, "_list_build_processes", lambda: [])
    monkeypatch.setattr(ps, "_loadavg_1m", lambda: 2.5)
    monkeypatch.setattr(ps, "_ncpu", lambda: 18)
    monkeypatch.setattr(ps, "_runnable_thread_count", lambda: 1)
    monkeypatch.setattr(ps, "_thermal_ok", lambda: (True, "ok"))
    q = ps.gather_quiescence()
    assert q["quiet"] is True
    assert q["reasons"] == []
    # The council-mandated NEW provenance fields are present.
    assert q["active_molt_processes"] == []
    assert q["active_cargo_or_rustc_processes"] == []
    assert q["loadavg_1m"] == 2.5
    assert q["ncpu"] == 18
    assert q["runnable_signal"] == 1


def test_gather_quiescence_not_quiet_when_build_active(monkeypatch) -> None:
    monkeypatch.setattr(
        ps, "_list_build_processes",
        lambda: [{"pid": 4242, "cmd": "cargo build -p molt-backend"}],
    )
    monkeypatch.setattr(ps, "_loadavg_1m", lambda: 2.0)
    monkeypatch.setattr(ps, "_ncpu", lambda: 18)
    monkeypatch.setattr(ps, "_runnable_thread_count", lambda: 1)
    monkeypatch.setattr(ps, "_thermal_ok", lambda: (True, "ok"))
    q = ps.gather_quiescence()
    assert q["quiet"] is False
    assert any("active build process" in r for r in q["reasons"])
    assert q["active_cargo_or_rustc_processes"]


def test_gather_quiescence_not_quiet_when_load_over_threshold(monkeypatch) -> None:
    # load 12 > 18*0.5=9 -> not quiet, even with no build process visible.
    monkeypatch.setattr(ps, "_list_build_processes", lambda: [])
    monkeypatch.setattr(ps, "_loadavg_1m", lambda: 12.0)
    monkeypatch.setattr(ps, "_ncpu", lambda: 18)
    monkeypatch.setattr(ps, "_runnable_thread_count", lambda: 1)
    monkeypatch.setattr(ps, "_thermal_ok", lambda: (True, "ok"))
    q = ps.gather_quiescence()
    assert q["quiet"] is False
    assert any("load" in r for r in q["reasons"])


def test_gather_quiescence_probe_failure_cannot_certify(monkeypatch) -> None:
    # If pgrep is unavailable we must NOT certify quiet (fail-closed authority).
    monkeypatch.setattr(
        ps, "_list_build_processes",
        lambda: [{"pid": -1, "cmd": "pgrep-unavailable", "probe_failed": True}],
    )
    monkeypatch.setattr(ps, "_loadavg_1m", lambda: 1.0)
    monkeypatch.setattr(ps, "_ncpu", lambda: 18)
    monkeypatch.setattr(ps, "_runnable_thread_count", lambda: 0)
    monkeypatch.setattr(ps, "_thermal_ok", lambda: (True, "ok"))
    q = ps.gather_quiescence()
    assert q["quiet"] is False
    assert any("probe" in r for r in q["reasons"])


def test_gather_quiescence_runnable_storm_flags_contention(monkeypatch) -> None:
    # Many runnable threads (build storm load not yet caught by EWMA) -> not quiet.
    monkeypatch.setattr(ps, "_list_build_processes", lambda: [])
    monkeypatch.setattr(ps, "_loadavg_1m", lambda: 2.0)
    monkeypatch.setattr(ps, "_ncpu", lambda: 18)
    monkeypatch.setattr(ps, "_runnable_thread_count", lambda: 30)
    monkeypatch.setattr(ps, "_thermal_ok", lambda: (True, "ok"))
    q = ps.gather_quiescence()
    assert q["quiet"] is False
    assert any("runnable" in r for r in q["reasons"])


def test_require_quiescent_forces_nonauthoritative(monkeypatch) -> None:
    # --require-quiescent + a non-quiet machine => provenance.authoritative False
    # even on a clean origin/main tree.
    monkeypatch.setattr(ps, "_git_output", lambda args: _fake_git(args))
    monkeypatch.setattr(
        ps, "_benchmark_tool_identity",
        lambda: {"ondisk_blob_sha": "b", "modified_vs_head": "false",
                 "last_commit_sha": "c"},
    )
    monkeypatch.setattr(ps, "_stdlib_cache_key_signal", lambda: "deadbeef")
    noisy = {
        "quiet": False, "reasons": ["1 active build process(es): 1:cargo"],
        "active_molt_processes": [], "active_cargo_or_rustc_processes": [{"pid": 1}],
        "loadavg_1m": 12.0, "ncpu": 18, "runnable_signal": 5,
        "loadavg_threshold": 9.0, "thermal_ok": True, "thermal_note": None,
    }
    prov = ps.gather_provenance(None, quiescence=noisy, require_quiescent=True)
    assert prov["authoritative"] is False
    assert "quiescent" in prov["authoritative_reason"].lower()
    # Without --require-quiescent the SAME noisy machine does not block authority
    # (the machine state is still recorded, just not gating).
    prov2 = ps.gather_provenance(None, quiescence=noisy, require_quiescent=False)
    assert prov2["authoritative"] is True
    assert prov2["quiescent"] is False


# --- Cycle attribution (Rule 1: cycles, not alloc-count) --------------------


def test_parse_sample_heaviest_reads_self_time_leaderboard(tmp_path) -> None:
    # Real macOS `sample` format: the self-sample COUNT is the TRAILING token,
    # "<symbol>  (in <lib>)        <count>" (verified against /usr/bin/sample).
    sample_out = (
        "Analysis of sampling molt-backend (pid 123) every 1 millisecond\n"
        "Sort by top of stack, same collapsed (when >= 5):\n"
        "        molt_dict_lookup  (in bench_x)        4200\n"
        "        molt_str_slice  (in bench_x)        1900\n"
        "        malloc  (in libsystem_malloc.dylib)        300\n"
        "\n"
        "Binary Images:\n"
        "       0x1000 - 0x2000 +bench_x ... /tmp/bench_x\n"
    )
    f = tmp_path / "sample.txt"
    f.write_text(sample_out, encoding="utf-8")
    top = ps._parse_sample_heaviest(f, top_n=25)
    assert len(top) == 3
    assert top[0]["symbol"] == "molt_dict_lookup"
    assert top[0]["self_samples"] == 4200
    assert top[0]["lib"] == "bench_x"
    assert top[2]["symbol"] == "malloc"
    assert top[2]["self_samples"] == 300


def test_parse_sample_heaviest_no_lib_form(tmp_path) -> None:
    # A leaderboard line with no "(in lib)" still parses (count trailing).
    sample_out = (
        "Sort by top of stack, same collapsed (when >= 5):\n"
        "        my_hot_symbol        512\n"
        "\n"
    )
    f = tmp_path / "nolib.txt"
    f.write_text(sample_out, encoding="utf-8")
    top = ps._parse_sample_heaviest(f, top_n=25)
    assert len(top) == 1
    assert top[0]["symbol"] == "my_hot_symbol"
    assert top[0]["self_samples"] == 512
    assert top[0]["lib"] is None


def test_parse_sample_heaviest_missing_section_is_empty(tmp_path) -> None:
    f = tmp_path / "nosort.txt"
    f.write_text("no leaderboard here\nBinary Images:\n", encoding="utf-8")
    assert ps._parse_sample_heaviest(f, top_n=25) == []


def test_capture_cycle_profile_documents_unavailable_sampler(monkeypatch) -> None:
    # When /usr/bin/sample is absent we return a DOCUMENTED note, never a fake
    # signal and never a crash (Rule 1's fallback).
    monkeypatch.setattr(ps, "_resolve_sampler", lambda: None)
    out = ps.capture_cycle_profile(
        ["/tmp/nonexistent-bin"], env={}, rss_mb=512, timeout_s=5,
    )
    assert out["available"] is False
    assert out["top_symbols"] == []
    assert "unavailable" in out["note"]


# --- print-provenance smoke (the function that was missing) -----------------


def test_print_provenance_emits_all_new_fields(capsys, monkeypatch) -> None:
    monkeypatch.setattr(ps, "_git_output", lambda args: _fake_git(args))
    monkeypatch.setattr(
        ps, "_benchmark_tool_identity",
        lambda: {"ondisk_blob_sha": "toolsha", "modified_vs_head": "false",
                 "last_commit_sha": "c"},
    )
    monkeypatch.setattr(ps, "_stdlib_cache_key_signal", lambda: "deadbeef")
    q = {
        "quiet": True, "reasons": [],
        "active_molt_processes": [], "active_cargo_or_rustc_processes": [],
        "loadavg_1m": 2.5, "ncpu": 18, "runnable_signal": 1,
        "loadavg_threshold": 9.0, "thermal_ok": True, "thermal_note": "ok",
    }
    prov = ps.gather_provenance(None, quiescence=q, require_quiescent=True)
    ps._print_provenance(prov)
    out = capsys.readouterr().out
    for field in (
        "origin_sha", "candidate_sha", "dirty_tree", "stdlib_cache_key",
        "backend_binary_identity", "active_molt_processes",
        "active_cargo_or_rustc_processes", "loadavg_1m", "ncpu", "runnable_signal",
    ):
        assert field in out
