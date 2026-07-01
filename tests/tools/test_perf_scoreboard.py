"""Unit tests for the two-dimensional perf-scoreboard classification + gate.

These exercise the verdict logic, provenance, and gate exit code with SYNTHETIC
cells — no molt rebuild required (the council's classification-logic test does
not need real benchmark runs). The measurement path itself is covered by the
tool's ``--self-test`` (one real bench_fib build).
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
for _p in (REPO_ROOT / "tools", REPO_ROOT / "src"):
    if str(_p) not in sys.path:
        sys.path.insert(0, str(_p))

import perf_scoreboard as ps  # noqa: E402
import perf_scoreboard_measure as measure  # noqa: E402
import harness_memory_guard  # noqa: E402


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
    c.log_artifact = f"bench/scoreboard/logs/{Path(benchmark).stem}.log"
    if build_ok:
        c.binary_size_kib = 512.0
        c.compile_time_s = 0.4
    if build_ok and molt_ok:
        c.molt_peak_rss_mib = 18.0
    if cpython_ok:
        c.cpython_peak_rss_mib = 15.0
    if build_ok and molt_ok and cpython_ok and not run_blocked:
        c.output_parity = True
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
    assert ps.verdict_fails_gate(c.verdict) is False


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
    assert ps.verdict_fails_gate(c.verdict) is True
    assert c.suspected_missing_fact  # a triage hint is attached
    assert c.fact_class == "repr_tir_type_lattice"
    assert c.attribution_confidence == 0.35


def test_fail_engine_uses_cycle_profile_fact_attribution() -> None:
    c = _cell(
        benchmark="tests/benchmarks/bench_exception_heavy.py",
        warm_molt_s=0.20,
        warm_cpython_s=0.12,
        cold_molt_s=0.30,
        cold_cpython_s=0.30,
    )
    c.cycle_profile = {
        "available": True,
        "in_binary_top": [
            {
                "symbol": "molt_runtime::builtins::exceptions::record_exception",
                "self_samples": 70,
            },
            {"symbol": "molt_inc_ref_obj", "self_samples": 40},
        ],
    }
    c.finalize(budget_ms=1000.0, authoritative=True)

    assert c.verdict == ps.VERDICT_FAIL_ENGINE
    assert c.fact_class == "exception_region"
    assert c.suspected_missing_fact == "ExceptionRegion/ownership"
    assert c.pypy_advantage_class == "borrow_inference"
    assert c.attribution_confidence is not None and c.attribution_confidence > 0.5


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
    assert ps.verdict_fails_gate(c.verdict) is False


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
    assert ps.verdict_fails_gate(c.verdict) is True
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
    assert ps.verdict_fails_gate(c.verdict) is False


def test_fail_stale_overrides_everything() -> None:
    # A green-looking cell on a non-authoritative board is FAIL_STALE.
    c = _cell(
        warm_molt_s=0.10,
        warm_cpython_s=0.20,
        cold_molt_s=0.12,
        cold_cpython_s=0.25,
    )
    c.finalize(budget_ms=100.0, authoritative=False)
    assert c.verdict == ps.VERDICT_FAIL_STALE
    assert c.note == ps.NON_AUTHORITATIVE_NOTE
    assert ps.verdict_fails_gate(c.verdict) is True


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
    assert ps.verdict_fails_gate(c.verdict) is True


def test_build_failed_and_run_error_and_blocked_and_incompat() -> None:
    bf = _cell(build_ok=False)
    bf.finalize(authoritative=True)
    assert bf.verdict == ps.VERDICT_BUILD_FAILED
    assert ps.verdict_fails_gate(bf.verdict) is True

    re = _cell(molt_ok=False)
    re.finalize(authoritative=True)
    assert re.verdict == ps.VERDICT_RUN_ERROR
    assert ps.verdict_fails_gate(re.verdict) is True

    rb = _cell(run_blocked=True)
    rb.finalize(authoritative=True)
    assert rb.verdict == ps.VERDICT_RUN_BLOCKED
    assert ps.verdict_fails_gate(rb.verdict) is False

    ci = _cell(cpython_ok=False)
    ci.finalize(authoritative=True)
    assert ci.verdict == ps.VERDICT_CPY_INCOMPAT
    assert ps.verdict_fails_gate(ci.verdict) is False


# --- Gate exit code ---------------------------------------------------------


def _board(cells: list[ps.Cell], provenance: dict | None = None) -> dict:
    full_provenance = {
        "origin_sha": "a" * 40,
        "local_head_sha": "a" * 40,
        "merge_base_sha": "a" * 40,
        "dirty_tree": False,
        "benchmark_tool_sha": "b" * 40,
        "backend_binary_identity": {"native/release-fast": "x|1|2"},
        "stdlib_cache_key": "deadbeef",
        "authoritative": True,
    }
    if provenance is not None:
        full_provenance.update(provenance)
    return ps.build_scoreboard_doc(
        cells,
        benchmarks_run=[c.benchmark for c in cells],
        benchmarks_deferred=[],
        cpython_version="3.14",
        samples=5,
        warmup=2,
        provenance=full_provenance,
        cpython_identity={
            "cmd": ["test-python"],
            "executable": "test-python",
            "implementation": "CPython",
            "version": "3.14",
            "sys_platform": sys.platform,
            "machine": ps.platform.machine(),
            "arch": ps._host_arch(),
            "pointer_bits": ps._host_pointer_bits(),
        },
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
    problems = ps.validate_board(doc)
    assert problems == [], f"schema problems: {problems}"
    assert doc["provenance"]["origin_sha"] == "a" * 40
    assert doc["summary"]["cells_green"] == 1
    assert doc["summary"]["gate_fails"] is False


def test_scoreboard_writer_validates_before_emit(tmp_path: Path) -> None:
    g = _cell(
        warm_molt_s=0.10, warm_cpython_s=0.20, cold_molt_s=0.11, cold_cpython_s=0.25
    )
    g.finalize(budget_ms=1000.0, authoritative=True)
    doc = _board([g])

    out = tmp_path / "scoreboard.json"
    ps._write_scoreboard_doc(out, doc, context="unit-valid")

    written = json.loads(out.read_text(encoding="utf-8"))
    assert written["summary"]["cells_green"] == 1

    invalid = dict(doc)
    invalid.pop("schema_version")
    invalid_out = tmp_path / "invalid" / "scoreboard.json"
    with pytest.raises(ps.ScoreboardSchemaError) as exc:
        ps._write_scoreboard_doc(invalid_out, invalid, context="unit-invalid")
    assert "schema_version" in str(exc.value)
    assert not invalid_out.exists()

    invalid_cell_doc = json.loads(json.dumps(doc))
    only_cell = next(iter(ps.flatten_cells(invalid_cell_doc)))
    only_cell["log_artifact"] = ""
    with pytest.raises(ps.ScoreboardSchemaError) as cell_exc:
        ps._write_scoreboard_doc(
            tmp_path / "bad-cell.json", invalid_cell_doc, context="unit-bad-cell"
        )
    assert any("log_artifact" in problem for problem in cell_exc.value.problems)

    atomic_out = tmp_path / "scoreboard.partial.json"
    atomic_out.write_text("old-board\n", encoding="utf-8")
    invalid_atomic_doc = json.loads(json.dumps(doc))
    only_atomic_cell = next(iter(ps.flatten_cells(invalid_atomic_doc)))
    only_atomic_cell["compile_time_s"] = None
    with pytest.raises(ps.ScoreboardSchemaError) as atomic_exc:
        ps._write_scoreboard_doc_atomic(
            atomic_out, invalid_atomic_doc, context="unit-invalid-checkpoint"
        )
    assert atomic_out.read_text(encoding="utf-8") == "old-board\n"
    assert not atomic_out.with_suffix(".tmp").exists()
    assert any("compile_time_s" in problem for problem in atomic_exc.value.problems)


def test_measure_cell_records_molt_failure_payload_without_live_build(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    script = tmp_path / "bench_fib.py"
    script.write_text("print('not built')\n", encoding="utf-8")
    failure = measure.bench.MoltFailure(
        phase="build",
        status="daemon_crash",
        returncode=1,
        timed_out=False,
        elapsed_s=2.5,
        detail="backend_daemon_empty_response",
        message="backend daemon returned empty response",
        stderr="backend daemon returned empty response",
    )

    monkeypatch.setattr(measure, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(measure, "_perfscore_build_env", lambda spec: {})
    monkeypatch.setattr(
        measure.bench_suites,
        "canonical_benchmark_key",
        lambda _script: "tests/benchmarks/bench_fib.py",
    )
    monkeypatch.setattr(
        measure.bench_suites,
        "molt_args_for_benchmark",
        lambda _script: [],
    )
    monkeypatch.setattr(
        measure.bench,
        "prepare_molt_binary",
        lambda *args, **kwargs: failure,
    )

    cell = measure.measure_cell(
        script_path=script,
        spec=measure.BackendSpec("native", "llvm", "llvm", "native"),
        profile="release-fast",
        samples=1,
        warmup=0,
        rss_mb=64,
        timeout_s=1.0,
        batch_server=None,
        cpython_cmd=(sys.executable,),
        log_dir=tmp_path / "logs",
    )
    doc = _board([cell])

    assert cell.verdict == ps.VERDICT_BUILD_FAILED
    assert cell.molt_failure_phase == "build"
    assert cell.molt_failure_status == "daemon_crash"
    assert cell.molt_failure_detail == "backend_daemon_empty_response"
    assert "backend daemon returned empty response" in (cell.molt_failure_message or "")
    assert ps.validate_board(doc) == []
    log_text = (tmp_path / cell.log_artifact).read_text(encoding="utf-8")
    assert "BUILD FAILURE MESSAGE: backend daemon returned empty response" in log_text


def _oracle_payload(
    *,
    implementation: str = "CPython",
    version: str = "3.14.0",
    executable: str = "candidate-real-python",
    sys_platform: str | None = None,
    machine: str | None = None,
    pointer_bits: int | None = None,
) -> str:
    return json.dumps(
        {
            "implementation": implementation,
            "version": version,
            "executable": executable,
            "sys_platform": sys_platform or sys.platform,
            "machine": machine or ps.platform.machine(),
            "pointer_bits": pointer_bits or ps._host_pointer_bits(),
        }
    )


def test_cpython_oracle_resolver_requires_host_native_cpython(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    wrong_machine = "arm64" if ps._host_arch() != "aarch64" else "x86_64"
    candidates = [
        ("store-python3",),
        ("pypy",),
        ("old-python",),
        ("wrong-arch-python",),
        ("good-python",),
    ]
    monkeypatch.setattr(ps, "_default_cpython_candidate_cmds", lambda: candidates)
    monkeypatch.setattr(
        ps.bench, "_canonical_interpreter", lambda executable: f"resolved:{executable}"
    )

    def fake_metadata_probe(cmd, **kwargs):
        name = str(cmd[0]).removeprefix("resolved:")
        if name == "store-python3":
            return SimpleNamespace(
                returncode=9009,
                stdout="",
                stderr="Python was not found",
            )
        if name == "pypy":
            return SimpleNamespace(
                returncode=0,
                stdout=_oracle_payload(implementation="PyPy", executable="pypy-real"),
                stderr="",
            )
        if name == "old-python":
            return SimpleNamespace(
                returncode=0,
                stdout=_oracle_payload(version="3.11.9", executable="old-real"),
                stderr="",
            )
        if name == "wrong-arch-python":
            return SimpleNamespace(
                returncode=0,
                stdout=_oracle_payload(
                    executable="wrong-arch-real", machine=wrong_machine
                ),
                stderr="",
            )
        assert name == "good-python"
        return SimpleNamespace(
            returncode=0,
            stdout=_oracle_payload(executable="good-real-python"),
            stderr="",
        )

    monkeypatch.setattr(ps, "_metadata_probe", fake_metadata_probe)

    oracle = ps._resolve_system_cpython(None)

    assert oracle.cmd == ("resolved:good-real-python",)
    assert oracle.implementation == "CPython"
    assert oracle.version == "3.14.0"
    assert oracle.sys_platform == sys.platform
    assert oracle.arch == ps._host_arch()
    assert oracle.pointer_bits == ps._host_pointer_bits()


def test_cpython_oracle_windows_launcher_resolves_to_real_interpreter(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        ps, "_default_cpython_candidate_cmds", lambda: [("py", "-3.14")]
    )
    monkeypatch.setattr(
        ps.bench, "_canonical_interpreter", lambda executable: f"resolved:{executable}"
    )

    def fake_metadata_probe(cmd, **kwargs):
        assert cmd[:2] == ["resolved:py", "-3.14"]
        return SimpleNamespace(
            returncode=0,
            stdout=_oracle_payload(executable="C:/Python314/python.exe"),
            stderr="",
        )

    monkeypatch.setattr(ps, "_metadata_probe", fake_metadata_probe)

    oracle = ps._resolve_system_cpython(None)

    assert oracle.cmd == ("resolved:C:/Python314/python.exe",)
    assert oracle.executable == "resolved:C:/Python314/python.exe"


def test_cpython_oracle_explicit_bad_baseline_fails_closed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        ps.bench, "_canonical_interpreter", lambda executable: executable
    )
    monkeypatch.setattr(
        ps,
        "_metadata_probe",
        lambda cmd, **kwargs: SimpleNamespace(
            returncode=0,
            stdout=_oracle_payload(implementation="PyPy", executable="pypy-real"),
            stderr="",
        ),
    )

    with pytest.raises(RuntimeError, match="explicit --cpython"):
        ps._resolve_system_cpython("pypy")


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


def test_codon_build_uses_benchmark_memory_guard(monkeypatch, tmp_path) -> None:
    calls: list[dict] = []

    def _fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": cmd, **kwargs})
        out_path = Path(cmd[cmd.index("-o") + 1])
        out_path.write_bytes(b"codon-binary")
        return SimpleNamespace(
            returncode=0,
            stdout="",
            stderr="",
            timed_out=False,
        )

    monkeypatch.setattr(
        ps.harness_memory_guard,
        "guarded_completed_process",
        _fake_guarded_completed_process,
    )
    monkeypatch.setattr(ps, "_measure_codon_warm", lambda *args, **kwargs: 0.007)

    runner = ps.CodonRunner(str(tmp_path / "bin" / "codon"))
    runner._tmp_root = tmp_path
    cell = _cell(benchmark="tests/benchmarks/bench_fib.py")
    log_lines: list[str] = []

    runner.measure_into(
        cell,
        script_path=REPO_ROOT / "tests" / "benchmarks" / "bench_fib.py",
        run_args=[],
        samples=1,
        warmup=0,
        rss_mb=512,
        timeout_s=3.0,
        log_lines=log_lines,
    )

    assert cell.codon_equivalent is True
    assert cell.codon_warm_s == 0.007
    assert len(calls) == 1
    call = calls[0]
    assert call["prefix"] == "MOLT_BENCH"
    assert call["cwd"] == ps.REPO_ROOT
    assert call["capture_output"] is True
    assert call["text"] is True
    assert call["timeout"] == 300
    assert call["cmd"][:3] == [str(tmp_path / "bin" / "codon"), "build", "-release"]


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
        0.60,
        stable=True,
        repeat_stability="STABLE_BELOW",
        repeat_ci=(0.58, 0.62),
        repeat_passes=5,
    )
    cls, reason = ps.classify_cell(c, quiescent=True)
    assert cls == ps.CLASS_RED_STABLE
    assert "TRUE compiler target" in reason


def test_classify_red_noisy_when_not_quiescent() -> None:
    # Same numbers but the machine was NOT quiet -> demoted to RED_NOISY, and the
    # reason NAMES contamination (this is exactly the "0.66 under load" artifact).
    c = _warm_cell(
        0.66,
        stable=True,
        repeat_stability="STABLE_BELOW",
        repeat_ci=(0.62, 0.70),
        repeat_passes=5,
    )
    cls, reason = ps.classify_cell(c, quiescent=False)
    assert cls == ps.CLASS_RED_NOISY
    assert "NOT quiescent" in reason


def test_classify_red_noisy_when_unstable() -> None:
    c = _warm_cell(
        0.60,
        stable=False,
        repeat_stability="STABLE_BELOW",
        repeat_ci=(0.58, 0.62),
        repeat_passes=5,
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
        0.98,
        stable=True,
        repeat_stability="STABLE_ABOVE",
        repeat_ci=(1.01, 1.10),
        repeat_passes=5,
    )
    cls, _ = ps.classify_cell(c, quiescent=True)
    assert cls == ps.CLASS_GREEN


# --- 5-state classification: TIE -------------------------------------------


def test_classify_tie_when_ci_straddles() -> None:
    c = _warm_cell(
        0.95,
        stable=True,
        repeat_stability="STRADDLES",
        repeat_ci=(0.85, 1.15),
        repeat_passes=5,
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
        2.50,
        stable=True,
        repeat_stability="STABLE_ABOVE",
        repeat_ci=(2.40, 2.60),
        repeat_passes=5,
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
        0.97,
        stable=True,
        repeat_stability="STRADDLES",
        repeat_ci=(0.90, 1.05),
        repeat_passes=5,
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
    red = _warm_cell(
        0.60,
        stable=True,
        repeat_stability="STABLE_BELOW",
        repeat_ci=(0.58, 0.62),
        repeat_passes=5,
    )
    green = _warm_cell(
        2.0,
        stable=True,
        repeat_stability="STABLE_ABOVE",
        repeat_ci=(1.9, 2.1),
        repeat_passes=5,
    )
    green.benchmark = "tests/benchmarks/bench_y.py"
    cells = [red, green]
    ps.apply_classification(cells, quiescent=True)
    assert red.classification == ps.CLASS_RED_STABLE
    assert green.classification == ps.CLASS_GREEN
    assert red.measured_quiescent is True and green.measured_quiescent is True


def test_apply_classification_contaminated_demotes_all_reds() -> None:
    red = _warm_cell(
        0.60,
        stable=True,
        repeat_stability="STABLE_BELOW",
        repeat_ci=(0.58, 0.62),
        repeat_passes=5,
    )
    ps.apply_classification([red], quiescent=False)
    assert red.classification == ps.CLASS_RED_NOISY
    assert red.measured_quiescent is False


def test_apply_classification_asymmetry_red_noisy_but_green_survives() -> None:
    # On a NON-quiescent board, a warm RED is demoted to RED_NOISY (load can
    # manufacture a false red) but a clear warm GREEN STAYS GREEN_STABLE (load
    # can only have made the win look smaller — it is a conservative green). This
    # is the board-level twin of the asymmetry rule, exercised the way
    # --rebuild-summary re-applies it from a stored non-quiescent board.
    red = _warm_cell(
        0.60,
        stable=True,
        repeat_stability="STABLE_BELOW",
        repeat_ci=(0.58, 0.62),
        repeat_passes=5,
    )
    green = _warm_cell(
        10.5,
        stable=True,
        repeat_stability="STABLE_ABOVE",
        repeat_ci=(9.9, 10.9),
        repeat_passes=3,
    )
    green.benchmark = "tests/benchmarks/bench_sum.py"
    ps.apply_classification([red, green], quiescent=False)
    assert red.classification == ps.CLASS_RED_NOISY
    assert green.classification == ps.CLASS_GREEN
    assert green.measured_quiescent is False  # recorded, but not gating the green


# --- Quiescence gather (synthetic; monkeypatched probes) --------------------


def test_loadavg_uses_portable_os_probe_before_sysctl(monkeypatch) -> None:
    monkeypatch.setattr(ps.os, "getloadavg", lambda: (3.25, 2.0, 1.0), raising=False)
    monkeypatch.setattr(
        ps,
        "_metadata_probe",
        lambda *args, **kwargs: pytest.fail("sysctl fallback should not run"),
    )
    assert ps._loadavg_1m() == 3.25


def test_loadavg_falls_back_to_sysctl_when_os_probe_missing(monkeypatch) -> None:
    monkeypatch.delattr(ps.os, "getloadavg", raising=False)

    def fake_metadata_probe(cmd: list[str], **kwargs) -> SimpleNamespace:
        assert cmd == ["sysctl", "-n", "vm.loadavg"]
        return SimpleNamespace(stdout="{ 4.50 4.00 3.00 }")

    monkeypatch.setattr(ps, "_metadata_probe", fake_metadata_probe)
    assert ps._loadavg_1m() == 4.5


def test_ncpu_uses_portable_os_probe_before_sysctl(monkeypatch) -> None:
    monkeypatch.setattr(ps.os, "cpu_count", lambda: 6)
    monkeypatch.setattr(
        ps,
        "_metadata_probe",
        lambda *args, **kwargs: pytest.fail("sysctl fallback should not run"),
    )
    assert ps._ncpu() == 6


def test_ncpu_falls_back_to_sysctl_when_os_probe_missing(monkeypatch) -> None:
    monkeypatch.setattr(ps.os, "cpu_count", lambda: None)

    def fake_metadata_probe(cmd: list[str], **kwargs) -> SimpleNamespace:
        assert cmd == ["sysctl", "-n", "hw.ncpu"]
        return SimpleNamespace(stdout="12")

    monkeypatch.setattr(ps, "_metadata_probe", fake_metadata_probe)
    assert ps._ncpu() == 12


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
        ps,
        "_list_build_processes",
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
        ps,
        "_list_build_processes",
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
        ps,
        "_benchmark_tool_identity",
        lambda: {
            "ondisk_blob_sha": "b",
            "modified_vs_head": "false",
            "last_commit_sha": "c",
        },
    )
    monkeypatch.setattr(ps, "_stdlib_cache_key_signal", lambda: "deadbeef")
    noisy = {
        "quiet": False,
        "reasons": ["1 active build process(es): 1:cargo"],
        "active_molt_processes": [],
        "active_cargo_or_rustc_processes": [{"pid": 1}],
        "loadavg_1m": 12.0,
        "ncpu": 18,
        "runnable_signal": 5,
        "loadavg_threshold": 9.0,
        "thermal_ok": True,
        "thermal_note": None,
    }
    prov = ps.gather_provenance(None, quiescence=noisy, require_quiescent=True)
    assert prov["authoritative"] is False
    assert "quiescent" in prov["authoritative_reason"].lower()
    # Without --require-quiescent the SAME noisy machine does not block authority
    # (the machine state is still recorded, just not gating).
    prov2 = ps.gather_provenance(None, quiescence=noisy, require_quiescent=False)
    assert prov2["authoritative"] is True
    assert prov2["quiescent"] is False


# --- safe_run custody -------------------------------------------------------


def test_safe_run_json_uses_benchmark_memory_guard(monkeypatch) -> None:
    calls: list[dict] = []

    def _fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": cmd, **kwargs})
        return SimpleNamespace(
            returncode=0,
            stdout="child-output\n",
            stderr=(
                'SAFE_RUN {"status":"ok","exit":0,"elapsed_s":0.125,"peak_rss_mib":9}\n'
            ),
            timed_out=False,
        )

    monkeypatch.setattr(
        ps.harness_memory_guard,
        "guarded_completed_process",
        _fake_guarded_completed_process,
    )

    outcome = ps._safe_run_json(
        [sys.executable, "-c", "print('ok')"],
        env={},
        rss_mb=123,
        timeout_s=4.0,
        label="unit",
        capture_stdout=True,
    )

    assert outcome.ok is True
    assert outcome.elapsed_s == 0.125
    assert outcome.peak_rss_mib == 9.0
    assert outcome.stdout == "child-output\n"
    assert outcome.stdout_tail == "child-output\n"
    assert outcome.stderr_tail is not None
    assert "SAFE_RUN" in outcome.stderr_tail
    assert len(calls) == 1
    call = calls[0]
    assert call["prefix"] == "MOLT_BENCH"
    assert call["cwd"] == ps.REPO_ROOT
    assert call["capture_output"] is True
    assert call["text"] is True
    assert call["timeout"] == 34.0
    assert call["cmd"][:2] == [sys.executable, str(ps.SAFE_RUN)]
    assert "--json" in call["cmd"]
    assert "--rss-mb" in call["cmd"]
    assert "--timeout" in call["cmd"]


def test_profiling_popen_uses_benchmark_process_group(monkeypatch) -> None:
    calls: list[dict] = []

    def _fake_limits_from_env(prefix, env):
        calls.append({"limits_prefix": prefix, "limits_env": env})
        return "limits"

    def _fake_process_group_kwargs(limits, *, env):
        calls.append({"pg_limits": limits, "pg_env": env})
        return {"start_new_session": True}

    class _FakePopen:
        def __init__(self, cmd, **kwargs):
            calls.append({"cmd": cmd, **kwargs})

    monkeypatch.setattr(
        ps.harness_memory_guard,
        "limits_from_env",
        _fake_limits_from_env,
    )
    monkeypatch.setattr(
        ps.harness_memory_guard,
        "batch_process_group_kwargs",
        _fake_process_group_kwargs,
    )
    monkeypatch.setattr(ps.subprocess, "Popen", _FakePopen)

    proc = ps._profiling_popen(["sample", "target"], env={"K": "V"})

    assert isinstance(proc, _FakePopen)
    assert calls[0] == {"limits_prefix": "MOLT_BENCH", "limits_env": {"K": "V"}}
    assert calls[1] == {"pg_limits": "limits", "pg_env": {"K": "V"}}
    launch = calls[2]
    assert launch["cmd"] == ["sample", "target"]
    assert launch["env"] == {"K": "V"}
    assert launch["stdout"] == ps.subprocess.DEVNULL
    assert launch["stderr"] == ps.subprocess.DEVNULL
    assert launch["text"] is True
    assert launch["start_new_session"] is True


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
        ["/tmp/nonexistent-bin"],
        env={},
        rss_mb=512,
        timeout_s=5,
    )
    assert out["available"] is False
    assert out["top_symbols"] == []
    assert "unavailable" in out["note"]


# --- print-provenance smoke (the function that was missing) -----------------


def test_print_provenance_emits_all_new_fields(capsys, monkeypatch) -> None:
    monkeypatch.setattr(ps, "_git_output", lambda args: _fake_git(args))
    monkeypatch.setattr(
        ps,
        "_benchmark_tool_identity",
        lambda: {
            "ondisk_blob_sha": "toolsha",
            "modified_vs_head": "false",
            "last_commit_sha": "c",
        },
    )
    monkeypatch.setattr(ps, "_stdlib_cache_key_signal", lambda: "deadbeef")
    q = {
        "quiet": True,
        "reasons": [],
        "active_molt_processes": [],
        "active_cargo_or_rustc_processes": [],
        "loadavg_1m": 2.5,
        "ncpu": 18,
        "runnable_signal": 1,
        "loadavg_threshold": 9.0,
        "thermal_ok": True,
        "thermal_note": "ok",
    }
    prov = ps.gather_provenance(None, quiescence=q, require_quiescent=True)
    ps._print_provenance(prov)
    out = capsys.readouterr().out
    for field in (
        "origin_sha",
        "candidate_sha",
        "dirty_tree",
        "stdlib_cache_key",
        "backend_binary_identity",
        "active_molt_processes",
        "active_cargo_or_rustc_processes",
        "loadavg_1m",
        "ncpu",
        "runnable_signal",
    ):
        assert field in out


# ===========================================================================
# #76 warm-hot cycle attribution: inner-repeat + launch-dominance + refusal
# ===========================================================================

import perf_inner_repeat as ir  # noqa: E402


_LOOPABLE_BENCH = (
    "def main() -> None:\n"
    "    total = 0\n"
    "    for i in range(10):\n"
    "        total += i\n"
    "    print(total)\n"
    "\n\n"
    'if __name__ == "__main__":\n'
    "    main()\n"
)


# --- inner-repeat transform: the SEMANTICS-PRESERVING wrap ------------------


def test_inner_repeat_wraps_canonical_main_shape() -> None:
    plan = ir.analyze(_LOOPABLE_BENCH, inner_loops=50)
    assert plan.ok is True
    assert plan.inner_loops == 50
    assert plan.entry == "main"
    # The guard now loops main(); the rest of the program is intact.
    assert "for _ in range(50):" in plan.source
    assert "main()" in plan.source
    assert "def main(" in plan.source


def test_inner_repeat_output_is_one_shot_repeated_n_times() -> None:
    # The transform must be semantics-preserving: running it under CPython prints
    # the one-shot output exactly N times (proven without any molt build).
    import tempfile

    one = harness_memory_guard.guarded_completed_process(
        [sys.executable, "-c", _LOOPABLE_BENCH],
        prefix="MOLT_TEST",
        capture_output=True,
        text=True,
        timeout=30.0,
        cwd=REPO_ROOT,
    ).stdout
    plan = ir.analyze(_LOOPABLE_BENCH, inner_loops=4)
    with tempfile.NamedTemporaryFile("w", suffix=".py", delete=False) as f:
        f.write(plan.source)
        path = f.name
    try:
        looped = harness_memory_guard.guarded_completed_process(
            [sys.executable, path],
            prefix="MOLT_TEST",
            capture_output=True,
            text=True,
            timeout=30.0,
            cwd=REPO_ROOT,
        ).stdout
    finally:
        Path(path).unlink()
    assert looped == one * 4


def test_inner_repeat_refuses_when_main_mutates_module_state() -> None:
    # A `global` in main() makes a repeat accumulate into module state -> the
    # second iteration diverges. Must REFUSE (fail-closed), never wrap.
    src = (
        "g = 0\n"
        "def main() -> None:\n"
        "    global g\n"
        "    g += 1\n"
        "    print(g)\n"
        '\nif __name__ == "__main__":\n'
        "    main()\n"
    )
    plan = ir.analyze(src, inner_loops=10)
    assert plan.ok is False
    assert plan.source is None
    assert "global" in plan.reason


def test_inner_repeat_refuses_unrecognized_guard_body() -> None:
    # An extra statement in the guard would be multiplied incorrectly by the loop.
    src = (
        "def main() -> None:\n    print(1)\n"
        '\nif __name__ == "__main__":\n'
        "    print(0)\n    main()\n"
    )
    plan = ir.analyze(src, inner_loops=10)
    assert plan.ok is False
    assert "not exactly" in plan.reason


def test_inner_repeat_refuses_missing_guard() -> None:
    src = "def main() -> None:\n    print(1)\nmain()\n"
    plan = ir.analyze(src, inner_loops=10)
    assert plan.ok is False
    assert "guard" in plan.reason


def test_inner_repeat_refuses_main_with_required_args() -> None:
    src = (
        'def main(x) -> None:\n    print(x)\n\nif __name__ == "__main__":\n    main()\n'
    )
    plan = ir.analyze(src, inner_loops=10)
    assert plan.ok is False
    assert "argument" in plan.reason


def test_inner_repeat_refuses_n_below_two() -> None:
    plan = ir.analyze(_LOOPABLE_BENCH, inner_loops=1)
    assert plan.ok is False
    assert "< 2" in plan.reason


def test_inner_repeat_refuses_syntax_error() -> None:
    plan = ir.analyze("def main(:\n", inner_loops=10)
    assert plan.ok is False
    assert "does not parse" in plan.reason


def test_inner_repeat_refuses_two_main_defs() -> None:
    src = (
        "def main() -> None:\n    pass\n"
        "def main() -> None:\n    pass\n"
        '\nif __name__ == "__main__":\n    main()\n'
    )
    plan = ir.analyze(src, inner_loops=10)
    assert plan.ok is False
    assert "exactly one" in plan.reason


# --- launch-dominance classification + the REFUSAL gate ---------------------


def test_is_launch_frame_matches_dyld_start_only() -> None:
    assert ps._is_launch_frame("_dyld_start", "dyld") is True
    # A same-named symbol in the program binary is NOT launch (lib differs).
    assert ps._is_launch_frame("_dyld_start", "bench_x_molt") is False
    assert ps._is_launch_frame("molt_user_main", "bench_x_molt") is False


def test_classify_launch_dominance_one_shot_is_launch_dominated() -> None:
    # The one-shot shape from #69: _dyld_start swamps the leaderboard -> refuse.
    syms = [
        {"symbol": "_dyld_start", "self_samples": 170, "lib": "dyld"},
        {"symbol": "mach_msg2_trap", "self_samples": 16, "lib": "dyld"},
        {"symbol": "???", "self_samples": 6, "lib": "bench_x_molt"},
    ]
    bd = ps.classify_launch_dominance(syms)
    assert bd["launch_dominates"] is True
    assert bd["launch_fraction"] > 0.8


def test_classify_launch_dominance_looped_is_not_dominated() -> None:
    # After inner-repeat: in-binary frames dominate, launch is a sliver -> OK.
    syms = [
        {"symbol": "split_field_bounds", "self_samples": 148, "lib": "etl_molt"},
        {"symbol": "etl__molt_user_main", "self_samples": 140, "lib": "etl_molt"},
        {"symbol": "_dyld_start", "self_samples": 20, "lib": "dyld"},
    ]
    bd = ps.classify_launch_dominance(syms)
    assert bd["launch_dominates"] is False
    assert bd["launch_fraction"] < 0.10
    assert bd["total"] == 308  # 148 + 140 + 20
    assert bd["in_binary_samples"] == 288  # 148 + 140 (launch excluded)


def test_classify_launch_dominance_empty_refuses() -> None:
    bd = ps.classify_launch_dominance([])
    assert bd["launch_dominates"] is True  # no signal -> cannot attribute
    assert bd["total"] == 0


def test_classify_launch_dominance_at_threshold_refuses() -> None:
    # Exactly at the 40% refusal fraction is treated as dominated (>=).
    syms = [
        {"symbol": "_dyld_start", "self_samples": 40, "lib": "dyld"},
        {"symbol": "hot", "self_samples": 60, "lib": "bench_molt"},
    ]
    bd = ps.classify_launch_dominance(syms)
    assert bd["launch_fraction"] == 0.40
    assert bd["launch_dominates"] is True


def test_top_in_binary_frames_filters_to_the_binary() -> None:
    syms = [
        {"symbol": "__findenv_locked", "self_samples": 190, "lib": "libsystem_c.dylib"},
        {"symbol": "hot_a", "self_samples": 100, "lib": "bench_molt"},
        {"symbol": "_tlv_get_addr", "self_samples": 90, "lib": "libdyld.dylib"},
        {"symbol": "hot_b", "self_samples": 50, "lib": "bench_molt"},
    ]
    top = ps.top_in_binary_frames(syms, binary_lib="bench_molt", top_n=10)
    assert [t["symbol"] for t in top] == ["hot_a", "hot_b"]
    # leaderboard_pct is the share of the WHOLE leaderboard (430 total).
    assert abs(top[0]["leaderboard_pct"] - 100 * 100 / 430) < 0.05


# --- capture_hot_only_profile: documented fallbacks (no real build) ----------


def test_capture_hot_only_documents_unavailable_sampler(monkeypatch) -> None:
    monkeypatch.setattr(ps, "_resolve_sampler", lambda: None)
    out = ps.capture_hot_only_profile(
        Path("/tmp/nonexistent-bin"),
        run_args=[],
        env={},
        rss_mb=512,
        inner_loops=40,
    )
    assert out["available"] is False
    assert out["refused"] is True
    assert "unavailable" in out["refused_reason"]
    assert out["in_binary_top"] == []


def test_capture_hot_only_refuses_on_oom_with_leak_reason(monkeypatch) -> None:
    # If the inner-repeat amplifies a per-iteration leak past the RSS cap, the
    # size run OOMs -> refuse with the leak reason (never sample a dying process).
    monkeypatch.setattr(ps, "_resolve_sampler", lambda: "/usr/bin/sample")

    def _fake_size(cmd, *, env, rss_mb, timeout_s, label):
        return ps.RunOutcome(
            ok=False,
            elapsed_s=2.9,
            peak_rss_mib=float(rss_mb),
            status="oom",
            exit_code=137,
        )

    monkeypatch.setattr(ps, "_safe_run_json", _fake_size)
    out = ps.capture_hot_only_profile(
        Path("/tmp/whatever-bin"),
        run_args=[],
        env={},
        rss_mb=2048,
        inner_loops=400,
    )
    assert out["available"] is False
    assert out["refused"] is True
    assert out.get("leak_suspected") is True
    assert "LEAK" in out["refused_reason"]
    assert "LOWER --inner-repeat" in out["refused_reason"]
    assert out["size_status"] == "oom"
    assert out["size_exit_code"] == 137


def test_capture_hot_only_refusal_preserves_size_failure_tails(monkeypatch) -> None:
    monkeypatch.setattr(ps, "_resolve_sampler", lambda: "/usr/bin/sample")

    def _fake_size(cmd, *, env, rss_mb, timeout_s, label):
        return ps.RunOutcome(
            ok=False,
            elapsed_s=1.0,
            peak_rss_mib=64.0,
            status="nonzero",
            exit_code=2,
            stdout_tail="partial stdout\n",
            stderr_tail="traceback tail\n",
        )

    monkeypatch.setattr(ps, "_safe_run_json", _fake_size)
    out = ps.capture_hot_only_profile(
        Path("/tmp/whatever-bin"),
        run_args=[],
        env={},
        rss_mb=2048,
        inner_loops=40,
    )
    assert out["available"] is False
    assert out["refused"] is True
    assert out["size_status"] == "nonzero"
    assert out["size_exit_code"] == 2
    assert out["size_stdout_tail"] == "partial stdout\n"
    assert out["size_stderr_tail"] == "traceback tail\n"


def test_capture_hot_only_refuses_when_runtime_too_short(monkeypatch) -> None:
    # If the looped runtime cannot carve a steady window after warmup, refuse
    # with "increase --inner-repeat" (never sample a window that overruns exit).
    monkeypatch.setattr(ps, "_resolve_sampler", lambda: "/usr/bin/sample")

    def _fake_short(cmd, *, env, rss_mb, timeout_s, label):
        return ps.RunOutcome(
            ok=True,
            elapsed_s=0.4,
            peak_rss_mib=100.0,
            status="ok",
            exit_code=0,
        )

    monkeypatch.setattr(ps, "_safe_run_json", _fake_short)
    out = ps.capture_hot_only_profile(
        Path("/tmp/whatever-bin"),
        run_args=[],
        env={},
        rss_mb=2048,
        inner_loops=3,
        warmup_s=0.6,
        window_s=3.0,
    )
    assert out["available"] is False
    assert out["refused"] is True
    assert "increase --inner-repeat" in out["refused_reason"]
