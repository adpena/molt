#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import sys
import time
from dataclasses import asdict
from pathlib import Path
from typing import Any

_SRC_ROOT = Path(__file__).resolve().parents[1] / "src"
if str(_SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(_SRC_ROOT))

import bench  # noqa: E402
import bench_suites  # noqa: E402
import harness_memory_guard  # noqa: E402
from molt.dx import cargo_target_dir_for_artifact_root  # noqa: E402
from perf_schema import RED_THRESHOLD  # noqa: E402
from perf_scoreboard_model import (  # noqa: E402
    PERFSCORE_SESSION_ID,
    PROFILE_BUILD_FLAG,
    REPO_ROOT,
    RUN_BLOCKED_BACKENDS,
    BackendSpec,
    Cell,
    PhaseStats,
    _llvm_sys_prefix,
    _llvm_sys_prefix_env_var,
    _repeat_stability,
    _robust_cell_stable,
    _safe_ratio,
    _safe_run_json,
    _warm_speedup_ci,
)


def _perfscore_build_env(spec: BackendSpec) -> dict[str, str]:
    """Build the conformance/build env for a backend lane.

    Sets the constitution's session isolation + the LLVM_SYS prefix + the
    MOLT_BACKEND selector. bench._canonical_bench_env folds in the molt
    conformance env (PYTHONPATH, codec, conformance dirs).
    """
    base = os.environ.copy()
    base["MOLT_SESSION_ID"] = PERFSCORE_SESSION_ID
    base["CARGO_TARGET_DIR"] = str(
        cargo_target_dir_for_artifact_root(REPO_ROOT, PERFSCORE_SESSION_ID)
    )
    if spec.molt_backend is not None:
        base["MOLT_BACKEND"] = spec.molt_backend
    else:
        base.pop("MOLT_BACKEND", None)
    if spec.backend == "llvm":
        prefix = _llvm_sys_prefix()
        prefix_env_var = _llvm_sys_prefix_env_var()
        if prefix and prefix_env_var:
            base[prefix_env_var] = prefix
    env = bench._canonical_bench_env(base)
    return env


def _cpython_run_env() -> dict[str, str]:
    """Env for the CPython baseline — src on PYTHONPATH, deterministic hashing."""
    env = bench._base_python_env()
    env["MOLT_SESSION_ID"] = PERFSCORE_SESSION_ID
    return env


def measure_cell(
    *,
    script_path: Path,
    spec: BackendSpec,
    profile: str,
    samples: int,
    warmup: int,
    rss_mb: int,
    timeout_s: float,
    batch_server: bench._BenchBatchBuildServer | None,
    cpython_cmd: tuple[str, ...],
    log_dir: Path,
    budget_ms: float | None = None,
    authoritative: bool = True,
    pypy_bin: str | None = None,
    codon_runner: Any | None = None,
    repeat: int = 1,
    emit_cycle_profile: bool = False,
) -> Cell:
    """Build + time one (benchmark, target, backend, profile) cell.

    ``repeat`` (>=1) runs N independent warm measurement PASSES (each a full
    warmup+samples block for molt AND CPython) and attaches a per-pass
    warm_speedup CI; a verdict is STABLE only if the CI does not straddle 1.00.
    ``emit_cycle_profile`` captures a CPU CYCLE profile (``/usr/bin/sample``) for
    a warm-red cell — the Rule-1 attribution signal (cycles, not alloc-count).
    """
    benchmark = bench_suites.canonical_benchmark_key(script_path)
    cell = Cell(
        benchmark=benchmark, target=spec.target, backend=spec.backend, profile=profile
    )
    log_path = log_dir / f"{Path(benchmark).stem}__{spec.backend}__{profile}.log"
    cell.log_artifact = str(log_path.relative_to(REPO_ROOT))
    log_lines: list[str] = [f"# {benchmark} | {spec.backend} | {profile}"]

    build_env = _perfscore_build_env(spec)
    extra_args = bench_suites.molt_args_for_benchmark(script_path)
    build_flag = PROFILE_BUILD_FLAG.get(profile, "release")

    # --- Build the molt binary via the canonical daemon batch build ---------
    binary = None
    if spec.build_target == "native":
        try:
            binary = bench.prepare_molt_binary(
                str(script_path),
                extra_args=extra_args,
                env=build_env,
                build_profile=build_flag,
                batch_server=batch_server,
                build_timeout_s=600.0,
            )
        except Exception as exc:  # noqa: BLE001 - record, never crash the sweep
            log_lines.append(f"BUILD EXCEPTION: {exc!r}")
            binary = bench.classify_molt_process_failure(
                phase="build",
                returncode=None,
                stderr=repr(exc),
                elapsed_s=None,
                default_status="build_exception",
            )
    else:
        # WASM build/link only — produced via the CLI, not run here.
        binary = _build_wasm_only(script_path, build_env, build_flag, log_lines)

    if not isinstance(binary, bench.MoltBinary):
        cell.build_ok = False
        if isinstance(binary, bench.MoltFailure):
            _record_molt_failure(cell, binary)
            detail = f" detail={binary.detail}" if binary.detail else ""
            log_lines.append(f"BUILD FAILED status={binary.status}{detail}")
            if binary.message:
                log_lines.append(f"BUILD FAILURE MESSAGE: {binary.message}")
        else:
            _record_molt_failure(
                cell,
                bench.classify_molt_process_failure(
                    phase="build",
                    returncode=None,
                    stderr="Molt build returned no binary and no failure payload",
                    elapsed_s=None,
                    default_status="build_failed",
                ),
            )
            log_lines.append("BUILD FAILED")
        cell.finalize(budget_ms=budget_ms, authoritative=authoritative)
        _write_log(log_path, log_lines)
        return cell

    cell.build_ok = True
    cell.binary_size_kib = round(binary.size_kb, 1)
    cell.compile_time_s = round(binary.build_s, 3)
    log_lines.append(
        f"build_ok size_kib={cell.binary_size_kib} compile_s={cell.compile_time_s}"
    )

    if spec.backend in RUN_BLOCKED_BACKENDS:
        cell.run_blocked = True
        cell.run_blocked_reason = (
            "wasm run-path blocked: socket-import instantiation gap (build/link only)"
        )
        log_lines.append(f"RUN-BLOCKED: {cell.run_blocked_reason}")
        cell.finalize(budget_ms=budget_ms, authoritative=authoritative)
        _write_log(log_path, log_lines)
        _release_binary(binary)
        return cell

    run_args = bench.resolve_benchmark_run_args(str(script_path))
    molt_run_env = build_env

    # --- COLD samples (fresh cache; capture stdout once for parity) ---------
    cold_molt = _safe_run_json(
        [str(binary.path), *run_args],
        env=molt_run_env,
        rss_mb=rss_mb,
        timeout_s=timeout_s,
        label=f"molt-cold:{benchmark}",
        capture_stdout=True,
    )
    cold_cpy = _safe_run_json(
        [*cpython_cmd, str(script_path), *run_args],
        env=_cpython_run_env(),
        rss_mb=rss_mb,
        timeout_s=timeout_s,
        label=f"cpython-cold:{benchmark}",
        capture_stdout=True,
    )

    # --- WARM samples — N independent PASSES (council --repeat N) -----------
    # Each pass is a full warmup+samples block for molt AND CPython, yielding one
    # warm_speedup point estimate. The CANONICAL cell stats come from the pass
    # with the most molt samples (pass 1 in the common case); ALL passes feed the
    # confidence interval. A verdict is STABLE only if that CI clears 1.00.
    n_passes = max(1, repeat)
    per_pass_speedups: list[float] = []
    pass_results: list[tuple[PhaseStats, PhaseStats]] = []
    for pass_idx in range(n_passes):
        for _ in range(warmup):
            _safe_run_json(
                [str(binary.path), *run_args],
                env=molt_run_env,
                rss_mb=rss_mb,
                timeout_s=timeout_s,
                label=f"molt-warmup:{benchmark}",
            )
        molt_runs = [
            _safe_run_json(
                [str(binary.path), *run_args],
                env=molt_run_env,
                rss_mb=rss_mb,
                timeout_s=timeout_s,
                label=f"molt-warm:{benchmark}:p{pass_idx}",
            )
            for _ in range(samples)
        ]
        for _ in range(warmup):
            _safe_run_json(
                [*cpython_cmd, str(script_path), *run_args],
                env=_cpython_run_env(),
                rss_mb=rss_mb,
                timeout_s=timeout_s,
                label=f"cpython-warmup:{benchmark}",
            )
        cpy_runs = [
            _safe_run_json(
                [*cpython_cmd, str(script_path), *run_args],
                env=_cpython_run_env(),
                rss_mb=rss_mb,
                timeout_s=timeout_s,
                label=f"cpython-warm:{benchmark}:p{pass_idx}",
            )
            for _ in range(samples)
        ]
        m_stats = PhaseStats.from_runs(molt_runs)
        c_stats = PhaseStats.from_runs(cpy_runs)
        pass_results.append((m_stats, c_stats))
        sp = _safe_ratio(c_stats.median_s, m_stats.median_s)
        if sp is not None:
            per_pass_speedups.append(sp)

    # Canonical stats = the pass with the most molt samples (ties -> first).
    molt_stats, cpy_stats = max(
        pass_results, key=lambda pr: (pr[0].n, -pass_results.index(pr))
    )

    # --- CYCLE attribution for warm reds (Rule 1 — cycles, NOT alloc-count) --
    # Capture WHILE the binary still exists. A warm red is warm_speedup <= 1.00
    # i.e. cpython_warm <= molt_warm (the FAIL_ENGINE condition). We profile the
    # running molt binary so the next optimization is steered by CPU self-time,
    # never by an alloc count.
    warm_sp_now = _safe_ratio(cpy_stats.median_s, molt_stats.median_s)
    is_warm_red_now = (
        warm_sp_now is not None
        and warm_sp_now <= RED_THRESHOLD
        and molt_stats.n > 0
        and cpy_stats.n > 0
    )
    if emit_cycle_profile and is_warm_red_now:
        from perf_scoreboard import capture_cycle_profile

        log_lines.append("capturing CYCLE profile (warm red — Rule 1 attribution)")
        cell.cycle_profile = capture_cycle_profile(
            [str(binary.path), *run_args],
            env=molt_run_env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
        )
        avail = cell.cycle_profile.get("available")
        top = cell.cycle_profile.get("top_symbols") or []
        log_lines.append(
            f"cycle_profile available={avail} top={top[0]['symbol'] if top else '-'} "
            f"note={cell.cycle_profile.get('note')}"
        )

    _release_binary(binary)

    cell.molt_stats = asdict(molt_stats)
    cell.cpython_stats = asdict(cpy_stats)
    cell.molt_ok = molt_stats.n > 0 and cold_molt.ok
    cell.cpython_ok = cpy_stats.n > 0 and cold_cpy.ok

    # --- Repeat-pass CI (STABLE only if the CI does not straddle 1.00) -------
    if n_passes > 1 or per_pass_speedups:
        median_sp, var_sp, ci_lo, ci_hi = _warm_speedup_ci(per_pass_speedups)
        cell.repeat_passes = n_passes
        cell.repeat_warm_speedups = [round(s, 4) for s in per_pass_speedups]
        cell.repeat_median_warm = median_sp
        cell.repeat_variance = var_sp
        cell.repeat_ci_lo = ci_lo
        cell.repeat_ci_hi = ci_hi
        cell.repeat_stability = _repeat_stability(ci_lo, ci_hi)

    cell.cold_molt_s = round(cold_molt.elapsed_s, 6) if cold_molt.elapsed_s else None
    cell.cold_cpython_s = round(cold_cpy.elapsed_s, 6) if cold_cpy.elapsed_s else None
    cell.warm_molt_s = molt_stats.median_s
    cell.warm_cpython_s = cpy_stats.median_s

    cell.molt_peak_rss_mib = _max_opt(molt_stats.peak_rss_mib, cold_molt.peak_rss_mib)
    cell.cpython_peak_rss_mib = _max_opt(cpy_stats.peak_rss_mib, cold_cpy.peak_rss_mib)

    # Stability: molt is the ARTIFACT UNDER TEST and MUST be stable. CPython is
    # the reference floor; a single CPython GC/scheduler outlier (common on
    # multi-100ms benchmarks) must NOT invalidate a cell where molt wins
    # decisively and is itself stable — otherwise a won class (e.g.
    # class_hierarchy 8x, molt cv 0.03) gets masked UNSTABLE by one CPython
    # spike. So the cell is trustworthy iff molt is stable AND either CPython is
    # stable OR the warm verdict is ROBUST to CPython's full sample spread (the
    # warm_speedup stays on the same side of the 1.00 floor using BOTH CPython's
    # fastest and slowest sample). This is median-based + outlier-robust per
    # pyperf discipline, never a per-test special case.
    cell.stable = _robust_cell_stable(molt_stats, cpy_stats)

    # One-time output parity (informational; not the gate).
    if cold_molt.stdout is not None and cold_cpy.stdout is not None:
        cell.output_parity = cold_molt.stdout.strip() == cold_cpy.stdout.strip()

    if not cell.molt_ok:
        _record_molt_run_failure(cell, cold_molt, fallback_status="runtime_failed")
        cell.note = f"molt run unmeasurable (status={cold_molt.status})"
    elif not cell.cpython_ok:
        cell.note = f"cpython run unmeasurable (status={cold_cpy.status})"
    elif cell.stable and not cpy_stats.stable and molt_stats.stable:
        # Transparency: the cell is trusted despite CPython-side jitter because
        # molt is stable AND the verdict is robust to CPython's spread.
        cell.note = (
            f"cpython unstable (cv={cpy_stats.cv}) but verdict robust to its "
            f"spread; molt stable (cv={molt_stats.cv})"
        )

    # --- PyPy comparator lane (informational; never a hard gate) ------------
    # Only measured once per benchmark (lane-independent: PyPy runs the same
    # .py). We attach it to every backend cell for the same benchmark so the
    # column is populated, but it is a CPython-style interpreter comparator,
    # not a molt-backend fact.
    if pypy_bin is not None and cell.warm_molt_s:
        pypy_warm = _measure_interpreter_warm(
            pypy_bin,
            str(script_path),
            run_args,
            samples=samples,
            warmup=warmup,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"pypy:{benchmark}",
        )
        if pypy_warm is not None:
            cell.pypy_warm_s = round(pypy_warm, 6)
            cell.pypy_ratio = _safe_ratio(pypy_warm, cell.warm_molt_s)
            log_lines.append(
                f"pypy warm_median={cell.pypy_warm_s} ratio={cell.pypy_ratio}"
            )

    # --- Codon comparator lane (AOT north star; equivalence-gated) ----------
    if codon_runner is not None and cell.warm_molt_s:
        codon_runner.measure_into(
            cell,
            script_path=script_path,
            run_args=run_args,
            samples=samples,
            warmup=warmup,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            log_lines=log_lines,
        )

    cell.finalize(budget_ms=budget_ms, authoritative=authoritative)
    log_lines.append(
        f"molt warm_median={cell.warm_molt_s} cpython warm_median={cell.warm_cpython_s} "
        f"warm_speedup={cell.warm_speedup} cold_speedup={cell.cold_speedup} "
        f"startup_tax_ms={cell.startup_tax_ms} verdict={cell.verdict}"
    )
    _write_log(log_path, log_lines)
    return cell


def _record_molt_failure(cell: Cell, failure: bench.MoltFailure) -> None:
    payload = bench.molt_failure_payload(failure)
    cell.molt_failure = payload
    cell.molt_failure_phase = (
        payload["phase"] if isinstance(payload["phase"], str) else None
    )
    cell.molt_failure_status = (
        payload["status"] if isinstance(payload["status"], str) else None
    )
    cell.molt_failure_detail = (
        payload["detail"] if isinstance(payload["detail"], str) else None
    )
    cell.molt_failure_message = (
        payload["message"] if isinstance(payload["message"], str) else None
    )
    returncode = payload["returncode"]
    cell.molt_failure_returncode = returncode if isinstance(returncode, int) else None
    timed_out = payload["timed_out"]
    cell.molt_failure_timed_out = timed_out if isinstance(timed_out, bool) else False
    elapsed_s = payload["elapsed_s"]
    cell.molt_failure_elapsed_s = (
        float(elapsed_s) if isinstance(elapsed_s, (int, float)) else None
    )
    signal = payload["signal"]
    cell.molt_failure_signal = signal if isinstance(signal, dict) else None
    guard = payload["guard_violation"]
    cell.molt_failure_guard_violation = guard if isinstance(guard, dict) else None
    groups = payload["orphaned_process_groups"]
    cell.molt_failure_orphaned_process_groups = (
        [int(value) for value in groups]
        if isinstance(groups, list) and all(isinstance(value, int) for value in groups)
        else []
    )


def _record_molt_run_failure(
    cell: Cell,
    outcome: "RunOutcome",
    *,
    fallback_status: str,
) -> None:
    failure = bench.classify_molt_process_failure(
        phase="run",
        returncode=outcome.exit_code,
        stdout=outcome.stdout_tail,
        stderr=outcome.stderr_tail,
        elapsed_s=outcome.elapsed_s,
        timed_out=outcome.status == "timeout",
        default_status=fallback_status,
    )
    _record_molt_failure(cell, failure)


def _build_wasm_only(
    script_path: Path,
    build_env: dict[str, str],
    build_flag: str,
    log_lines: list[str],
) -> bench.MoltBinary | bench.MoltFailure:
    """Build+link a WASM artifact (run-path is blocked; we only verify it links).

    Uses the molt CLI directly with ``--target wasm``. Returns a MoltBinary-like
    handle whose ``path`` is the .wasm and whose size/compile-time are captured,
    so the scoreboard records build-facts for the WASM lane.
    """
    import tempfile

    out_dir = Path(
        tempfile.mkdtemp(prefix="perfscore-wasm-", dir=str(bench.BENCH_TMP_ROOT))
    )
    cmd = [
        *bench._molt_build_cmd(build_flag),
        "--target",
        "wasm",
        "--trusted",
        "--json",
        "--rebuild",
        "--out-dir",
        str(out_dir),
        str(script_path),
    ]
    start = time.perf_counter()
    try:
        res = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix="MOLT_BENCH",
            env=build_env,
            capture_output=True,
            text=True,
            timeout=600.0,
        )
    except Exception as exc:  # noqa: BLE001
        log_lines.append(f"WASM BUILD EXCEPTION: {exc!r}")
        return bench.classify_molt_process_failure(
            phase="build",
            returncode=None,
            stderr=repr(exc),
            elapsed_s=None,
            default_status="wasm_build_exception",
        )
    build_s = time.perf_counter() - start
    if res.returncode != 0:
        tail = (res.stderr or res.stdout or "").strip()[-2000:]
        log_lines.append(f"WASM BUILD FAILED rc={res.returncode}\n{tail}")
        return bench.classify_molt_process_failure(
            phase="build",
            returncode=res.returncode,
            stdout=res.stdout,
            stderr=res.stderr,
            elapsed_s=build_s,
            timed_out=bool(getattr(res, "timed_out", False)),
            default_status="wasm_build_failed",
        )
    try:
        payload = json.loads((res.stdout or "{}").strip() or "{}")
    except json.JSONDecodeError:
        log_lines.append("WASM BUILD: non-JSON stdout")
        return bench.classify_molt_process_failure(
            phase="build",
            returncode=res.returncode,
            stdout=res.stdout,
            stderr=res.stderr,
            elapsed_s=build_s,
            default_status="wasm_build_output_invalid",
        )
    out_str = payload.get("data", {}).get("output") or payload.get("output")
    if not out_str:
        # Fall back to scanning the out dir for a .wasm artifact.
        wasms = list(out_dir.rglob("*.wasm"))
        if not wasms:
            log_lines.append("WASM BUILD: no .wasm artifact")
            return bench.classify_molt_process_failure(
                phase="build",
                returncode=res.returncode,
                stdout=res.stdout,
                stderr="WASM BUILD: no .wasm artifact",
                elapsed_s=build_s,
                default_status="wasm_artifact_missing",
            )
        out_path = wasms[0]
    else:
        out_path = Path(out_str)
        if not out_path.exists():
            wasms = list(out_dir.rglob("*.wasm"))
            out_path = wasms[0] if wasms else out_path
    if not out_path.exists():
        log_lines.append(f"WASM BUILD: artifact missing {out_path}")
        return bench.classify_molt_process_failure(
            phase="build",
            returncode=res.returncode,
            stdout=res.stdout,
            stderr=f"WASM BUILD: artifact missing {out_path}",
            elapsed_s=build_s,
            default_status="wasm_artifact_missing",
        )
    size_kb = out_path.stat().st_size / 1024

    class _TmpHolder:
        def cleanup(self) -> None:
            import shutil

            shutil.rmtree(out_dir, ignore_errors=True)

    return bench.MoltBinary(out_path, _TmpHolder(), build_s, size_kb)


def _release_binary(binary: bench.MoltBinary) -> None:
    holder = getattr(binary, "temp_dir", None)
    if holder is not None:
        try:
            holder.cleanup()
        except Exception:  # noqa: BLE001
            pass


def _max_opt(a: float | None, b: float | None) -> float | None:
    vals = [v for v in (a, b) if v is not None]
    return max(vals) if vals else None


def _write_log(path: Path, lines: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def _measure_interpreter_warm(
    interp_bin: str,
    script: str,
    run_args: list[str],
    *,
    samples: int,
    warmup: int,
    rss_mb: int,
    timeout_s: float,
    label: str,
) -> float | None:
    """Warm steady-state median wall time for a Python-compatible interpreter.

    Same >=samples cold+warm discipline as the CPython path, through safe_run.
    Returns None if the interpreter cannot run the script (so a comparator that
    chokes on a benchmark simply leaves the column null, never poisons a cell).
    """
    env = _cpython_run_env()
    for _ in range(warmup):
        _safe_run_json(
            [interp_bin, script, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"{label}-warmup",
        )
    runs = [
        _safe_run_json(
            [interp_bin, script, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=label,
        )
        for _ in range(samples)
    ]
    stats = PhaseStats.from_runs(runs)
    return stats.median_s if stats.n > 0 else None


def _measure_codon_warm(
    codon_bin_path: str,
    run_args: list[str],
    *,
    samples: int,
    warmup: int,
    rss_mb: int,
    timeout_s: float,
    label: str,
    env: dict[str, str] | None = None,
) -> float | None:
    """Warm median wall time of a compiled Codon binary (safe_run-guarded)."""
    if env is None:
        env = _cpython_run_env()
    for _ in range(warmup):
        _safe_run_json(
            [codon_bin_path, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"{label}-warmup",
        )
    runs = [
        _safe_run_json(
            [codon_bin_path, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=label,
        )
        for _ in range(samples)
    ]
    stats = PhaseStats.from_runs(runs)
    return stats.median_s if stats.n > 0 else None
