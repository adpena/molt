#!/usr/bin/env python3
from __future__ import annotations

import os
from dataclasses import asdict
from pathlib import Path
from typing import Any

_SRC_ROOT = Path(__file__).resolve().parents[1] / "src"


from tools import bench  # noqa: E402
from tools import bench_suites  # noqa: E402
from tools.perf_schema import RED_THRESHOLD, output_parity_evidence  # noqa: E402
from tools.perf_scoreboard_model import (  # noqa: E402
    PERFSCORE_SESSION_ID,
    REPO_ROOT,
    BackendSpec,
    Cell,
    PhaseStats,
    RunOutcome,
    _llvm_sys_prefix,
    _llvm_sys_prefix_env_var,
    _repeat_stability,
    _robust_cell_stable,
    _safe_ratio,
    _run_measured_command,
    _warm_speedup_ci,
)


def _perfscore_build_env(spec: BackendSpec, profile: str) -> dict[str, str]:
    """Build the conformance/build env for a backend lane.

    Sets the constitution's session isolation and the LLVM_SYS prefix.
    bench._canonical_bench_env folds in the molt conformance env (PYTHONPATH,
    codec, conformance dirs). The codegen backend is a build flag
    from the canonical release lane, never an environment variable.
    """
    from tools.perf_scoreboard_build_profiles import profile_selection

    selection = profile_selection(spec, profile)
    base = os.environ.copy()
    base.update(selection.environment())
    # Scoreboard parity compares user-observable stderr. Profiler epochs emit
    # Molt-only diagnostics, so canonical timing runs explicitly disable ambient
    # profiler/leak modes instead of normalizing those diagnostics away later.
    base["MOLT_PROFILE"] = "0"
    base["MOLT_ASSERT_NO_LEAK"] = "0"
    base["MOLT_SESSION_ID"] = PERFSCORE_SESSION_ID
    # The canonical bench env derives this session's target dir under the
    # custody-selected artifact root; an ambient one would share build state.
    base.pop("MOLT_SESSION_ID_GENERATED", None)
    base.pop("MOLT_DIFF_CARGO_TARGET_DIR", None)
    base.pop("CARGO_TARGET_DIR", None)
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
    env.pop("MOLT_SESSION_ID_GENERATED", None)
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
    target_python_version: str,
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

    build_env = _perfscore_build_env(spec, profile)
    from tools.perf_scoreboard_build_profiles import profile_selection

    selection = profile_selection(spec, profile)
    extra_args = [
        "--python-version",
        target_python_version,
        "--backend",
        selection.codegen_backend,
        *bench_suites.molt_args_for_benchmark(script_path),
    ]
    build_flag = selection.guest_profile

    # One build/result owner for native and manifest-bound WASM deployments.
    binary = bench.prepare_molt_binary(
        str(script_path),
        extra_args=extra_args,
        env=build_env,
        build_profile=build_flag,
        build_target=spec.build_target,
        batch_server=batch_server,
        build_timeout_s=600.0,
    )

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

    primary: BaseException | None = None
    try:
        cell.build_ok = True
        cell.build_observation = binary.build_observation
        from tools.perf_scoreboard_build_profiles import profile_binding_problems

        binding_problems = profile_binding_problems(
            binary.build_observation, backend=spec.backend, profile=profile
        )
        if binding_problems:
            cell.note = "Unbound build profile: " + "; ".join(binding_problems)
            log_lines.append(cell.note)
            cell.finalize(budget_ms=budget_ms, authoritative=False)
            _write_log(log_path, log_lines)
            return cell
        cell.binary_size_kib = round(binary.size_kb, 1)
        cell.compile_time_s = round(binary.build_s, 3)
        log_lines.append(
            f"build_ok size_kib={cell.binary_size_kib} compile_s={cell.compile_time_s}"
        )

        run_args = bench.resolve_benchmark_run_args(str(script_path))
        from molt.node_runtime import NodeRuntimeError

        try:
            invocation = bench.molt_binary_invocation(binary, build_env)
        except NodeRuntimeError as exc:
            primary = exc
            _record_molt_failure(
                cell,
                bench.classify_molt_process_failure(
                    phase="run",
                    returncode=None,
                    stderr=str(exc),
                    elapsed_s=None,
                    default_status="run_host_unavailable",
                ),
            )
            cell.note = str(exc)
            log_lines.append(f"RUN HOST UNAVAILABLE: {exc}")
            cell.finalize(budget_ms=budget_ms, authoritative=authoritative)
            _write_log(log_path, log_lines)
            return cell
        molt_cmd, molt_run_env = invocation.command, invocation.environment

        # --- COLD samples (first process; retain exact streams for parity) ------
        cold_molt = _run_measured_command(
            [*molt_cmd, *run_args],
            env=molt_run_env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"molt-cold:{benchmark}",
            capture_output=True,
            guest_stderr_path=invocation.guest_stderr_path,
        )
        cold_cpy = _run_measured_command(
            [*cpython_cmd, str(script_path), *run_args],
            env=_cpython_run_env(),
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"cpython-cold:{benchmark}",
            capture_output=True,
        )
        failed_molt = None if cold_molt.ok else cold_molt
        failed_cpy = None if cold_cpy.ok else cold_cpy
        if cold_molt.host_stderr:
            log_lines.append(f"HOST STDERR molt:cold:\n{cold_molt.host_stderr}")
        molt_output_observations = [
            ("molt:cold", cold_molt.stdout, cold_molt.stderr, cold_molt.exit_code)
        ]
        cpy_output_observations = [
            ("cpython:cold", cold_cpy.stdout, cold_cpy.stderr, cold_cpy.exit_code)
        ]

        # --- WARM samples — N independent PASSES (council --repeat N) -----------
        # Each pass is a full warmup+samples block for molt AND CPython, yielding one
        # warm_speedup point estimate. The CANONICAL cell stats come from the pass
        # with the most molt samples (pass 1 in the common case); ALL passes feed the
        # confidence interval. Warmup subprocesses already execute the benchmark, so
        # their exact outputs also feed parity; otherwise a nondeterministic warmup
        # result could disappear from affirmative evidence. A verdict is STABLE only
        # if the CI clears 1.00.
        n_passes = max(1, repeat)
        per_pass_speedups: list[float] = []
        pass_results: list[tuple[PhaseStats, PhaseStats]] = []
        for pass_idx in range(n_passes):
            molt_warmups = [
                _run_measured_command(
                    [*molt_cmd, *run_args],
                    env=molt_run_env,
                    rss_mb=rss_mb,
                    timeout_s=timeout_s,
                    label=f"molt-warmup:{benchmark}:p{pass_idx}:s{sample_idx}",
                    capture_output=True,
                    guest_stderr_path=invocation.guest_stderr_path,
                )
                for sample_idx in range(warmup)
            ]
            log_lines.extend(
                f"HOST STDERR molt:warmup:p{pass_idx}:s{sample_idx}:\n{run.host_stderr}"
                for sample_idx, run in enumerate(molt_warmups)
                if run.host_stderr
            )
            molt_output_observations.extend(
                (
                    f"molt:warmup:p{pass_idx}:s{sample_idx}",
                    run.stdout,
                    run.stderr,
                    run.exit_code,
                )
                for sample_idx, run in enumerate(molt_warmups)
            )
            molt_runs = [
                _run_measured_command(
                    [*molt_cmd, *run_args],
                    env=molt_run_env,
                    rss_mb=rss_mb,
                    timeout_s=timeout_s,
                    label=f"molt-warm:{benchmark}:p{pass_idx}:s{sample_idx}",
                    capture_output=True,
                    guest_stderr_path=invocation.guest_stderr_path,
                )
                for sample_idx in range(samples)
            ]
            log_lines.extend(
                f"HOST STDERR molt:warm:p{pass_idx}:s{sample_idx}:\n{run.host_stderr}"
                for sample_idx, run in enumerate(molt_runs)
                if run.host_stderr
            )
            molt_output_observations.extend(
                (
                    f"molt:warm:p{pass_idx}:s{sample_idx}",
                    run.stdout,
                    run.stderr,
                    run.exit_code,
                )
                for sample_idx, run in enumerate(molt_runs)
            )
            cpy_warmups = [
                _run_measured_command(
                    [*cpython_cmd, str(script_path), *run_args],
                    env=_cpython_run_env(),
                    rss_mb=rss_mb,
                    timeout_s=timeout_s,
                    label=f"cpython-warmup:{benchmark}:p{pass_idx}:s{sample_idx}",
                    capture_output=True,
                )
                for sample_idx in range(warmup)
            ]
            cpy_output_observations.extend(
                (
                    f"cpython:warmup:p{pass_idx}:s{sample_idx}",
                    run.stdout,
                    run.stderr,
                    run.exit_code,
                )
                for sample_idx, run in enumerate(cpy_warmups)
            )
            cpy_runs = [
                _run_measured_command(
                    [*cpython_cmd, str(script_path), *run_args],
                    env=_cpython_run_env(),
                    rss_mb=rss_mb,
                    timeout_s=timeout_s,
                    label=f"cpython-warm:{benchmark}:p{pass_idx}:s{sample_idx}",
                    capture_output=True,
                )
                for sample_idx in range(samples)
            ]
            cpy_output_observations.extend(
                (
                    f"cpython:warm:p{pass_idx}:s{sample_idx}",
                    run.stdout,
                    run.stderr,
                    run.exit_code,
                )
                for sample_idx, run in enumerate(cpy_runs)
            )
            if failed_molt is None:
                failed_molt = next(
                    (
                        run
                        for group in (molt_warmups, molt_runs)
                        for run in group
                        if not run.ok
                    ),
                    None,
                )
            if failed_cpy is None:
                failed_cpy = next(
                    (
                        run
                        for group in (cpy_warmups, cpy_runs)
                        for run in group
                        if not run.ok
                    ),
                    None,
                )
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
            failed_molt is None
            and failed_cpy is None
            and warm_sp_now is not None
            and warm_sp_now <= RED_THRESHOLD
            and molt_stats.n > 0
            and cpy_stats.n > 0
        )
        if emit_cycle_profile and is_warm_red_now and spec.build_target != "native":
            cell.cycle_profile = {
                "available": False,
                "top_symbols": [],
                "note": "native cycle sampler cannot attribute WASM guest frames",
            }
        if emit_cycle_profile and is_warm_red_now and spec.build_target == "native":
            from tools.perf_scoreboard import capture_cycle_profile

            log_lines.append("capturing CYCLE profile (warm red — Rule 1 attribution)")
            cell.cycle_profile = capture_cycle_profile(
                [*molt_cmd, *run_args],
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

        cell.molt_stats = asdict(molt_stats)
        cell.cpython_stats = asdict(cpy_stats)
        cell.molt_ok = molt_stats.n > 0 and failed_molt is None
        cell.cpython_ok = cpy_stats.n > 0 and failed_cpy is None

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

        cell.cold_molt_s = (
            round(cold_molt.elapsed_s, 6) if cold_molt.elapsed_s else None
        )
        cell.cold_cpython_s = (
            round(cold_cpy.elapsed_s, 6) if cold_cpy.elapsed_s else None
        )
        cell.warm_molt_s = molt_stats.median_s
        cell.warm_cpython_s = cpy_stats.median_s

        cell.molt_peak_rss_mib = _max_opt(
            molt_stats.peak_rss_mib, cold_molt.peak_rss_mib
        )
        cell.cpython_peak_rss_mib = _max_opt(
            cpy_stats.peak_rss_mib, cold_cpy.peak_rss_mib
        )

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

        # Exact comparison plus per-runtime stability across the cold observation,
        # every existing warmup, and every timed warm sample. No extra benchmark
        # process is added.
        cell.output_parity = output_parity_evidence(
            reference_observations=cpy_output_observations,
            molt_observations=molt_output_observations,
        )

        if not cell.molt_ok:
            failure = failed_molt or cold_molt
            _record_molt_run_failure(cell, failure, fallback_status="runtime_failed")
            cell.note = f"molt run unmeasurable (status={failure.status})"
        elif not cell.cpython_ok:
            failure = failed_cpy or cold_cpy
            cell.note = f"cpython run unmeasurable (status={failure.status})"
        elif cell.output_parity.get("ok") is not True:
            reason = str(cell.output_parity.get("reason") or "comparison_unavailable")
            detail = str(cell.output_parity.get("detail") or reason)
            _record_molt_failure(
                cell,
                bench.MoltFailure(
                    phase="run",
                    status="output_mismatch",
                    returncode=cold_molt.exit_code,
                    timed_out=False,
                    elapsed_s=cold_molt.elapsed_s,
                    detail=reason,
                    message=detail,
                ),
            )
            log_lines.append(f"OUTPUT PARITY FAILED: {reason}: {detail}")
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
    except BaseException as exc:
        primary = exc
        raise
    finally:
        _release_binary(binary, primary)


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
    guard = outcome.guard_result
    failure = bench.classify_molt_process_failure(
        phase="run",
        returncode=outcome.exit_code,
        stdout=outcome.stdout_tail,
        stderr=outcome.stderr_tail,
        elapsed_s=outcome.elapsed_s,
        timed_out=outcome.status == "timeout",
        violation=guard.violation if guard is not None else None,
        orphaned_process_groups=guard.orphaned_process_groups
        if guard is not None
        else (),
        child_returncode=guard.child_returncode if guard is not None else None,
        infrastructure_failure=guard.infrastructure_failure
        if guard is not None
        else None,
        default_status=fallback_status,
    )
    _record_molt_failure(cell, failure)


def _release_binary(binary: bench.MoltBinary, primary: BaseException | None) -> None:
    bench._cleanup_bench_temp(binary.temp_dir, primary)


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
        run = _run_measured_command(
            [interp_bin, script, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"{label}-warmup",
        )
        if not run.ok:
            return None
    runs = [
        _run_measured_command(
            [interp_bin, script, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=label,
        )
        for _ in range(samples)
    ]
    if not all(run.ok for run in runs):
        return None
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
        run = _run_measured_command(
            [codon_bin_path, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"{label}-warmup",
        )
        if not run.ok:
            return None
    runs = [
        _run_measured_command(
            [codon_bin_path, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=label,
        )
        for _ in range(samples)
    ]
    if not all(run.ok for run in runs):
        return None
    stats = PhaseStats.from_runs(runs)
    return stats.median_s if stats.n > 0 else None
