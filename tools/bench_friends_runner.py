import statistics
from pathlib import Path

from bench_friends_env import _materialize_output_env_paths
from bench_friends_manifest import _resolve_env, _resolve_tokenized
from bench_friends_phase import (
    _extract_structured_elapsed,
    _metric_slug,
    _molt_failure_reason_suffix,
    _run_command,
)
from bench_friends_types import RunnerResult, RunnerSpec, SuiteSpec

import harness_memory_guard

def _run_prepare_steps(
    suite: SuiteSpec,
    *,
    suite_workdir: Path,
    suite_env: dict[str, str],
    tokens: dict[str, str],
    timeout_sec: int,
    logs_dir: Path,
    dry_run: bool,
    limits: harness_memory_guard.HarnessMemoryLimits,
) -> tuple[bool, str | None]:
    for idx, prepare_cmd in enumerate(suite.prepare_cmds, start=1):
        resolved_cmd = _resolve_tokenized(prepare_cmd, tokens)
        out = logs_dir / f"prepare_{idx}.stdout.log"
        err = logs_dir / f"prepare_{idx}.stderr.log"
        phase = _run_command(
            resolved_cmd,
            cwd=suite_workdir,
            env=suite_env,
            timeout_sec=timeout_sec,
            stdout_path=out,
            stderr_path=err,
            dry_run=dry_run,
            limits=limits,
            progress_label=f"suite={suite.id} phase=prepare step={idx}/{len(suite.prepare_cmds)}",
        )
        if not phase.ok:
            return False, f"prepare step {idx} failed"
    return True, None


def _run_runner(
    runner: RunnerSpec,
    *,
    suite: SuiteSpec,
    suite_workdir: Path,
    suite_env: dict[str, str],
    tokens: dict[str, str],
    logs_dir: Path,
    dry_run: bool,
    limits: harness_memory_guard.HarnessMemoryLimits,
) -> RunnerResult:
    if runner.skip_reason:
        return RunnerResult(
            name=runner.name,
            role=runner.role,
            status="skipped",
            reason=runner.skip_reason,
        )
    if not runner.run_cmd:
        return RunnerResult(
            name=runner.name,
            role=runner.role,
            status="skipped",
            reason="run_cmd not configured",
        )

    env = suite_env.copy()
    env.update(_resolve_env(runner.env, tokens))
    if not dry_run and (output_root := tokens.get("output_root")):
        _materialize_output_env_paths(env, output_root=Path(output_root))
    result = RunnerResult(name=runner.name, role=runner.role, status="ok")

    if runner.build_cmd:
        build_cmd = _resolve_tokenized(runner.build_cmd, tokens)
        build = _run_command(
            build_cmd,
            cwd=suite_workdir,
            env=env,
            timeout_sec=suite.timeout_sec,
            stdout_path=logs_dir / f"{runner.name}.build.stdout.log",
            stderr_path=logs_dir / f"{runner.name}.build.stderr.log",
            dry_run=dry_run,
            limits=limits,
            molt_failure_phase="build" if runner.name == "molt" else None,
            progress_label=f"suite={suite.id} runner={runner.name} phase=build",
        )
        result.build = build
        if not build.ok:
            result.status = "failed"
            result.molt_failure = build.molt_failure
            result.reason = (
                f"build failed{_molt_failure_reason_suffix(build.molt_failure)}"
            )
            return result

    run_cmd = _resolve_tokenized(runner.run_cmd, tokens)
    for run_idx in range(1, suite.repeat + 1):
        phase = _run_command(
            run_cmd,
            cwd=suite_workdir,
            env=env,
            timeout_sec=suite.timeout_sec,
            stdout_path=logs_dir / f"{runner.name}.run{run_idx}.stdout.log",
            stderr_path=logs_dir / f"{runner.name}.run{run_idx}.stderr.log",
            dry_run=dry_run,
            limits=limits,
            parse_stdout_json=runner.json_stdout,
            molt_failure_phase="run" if runner.name == "molt" else None,
            progress_label=(
                f"suite={suite.id} runner={runner.name} "
                f"phase=run repeat={run_idx}/{suite.repeat}"
            ),
        )
        result.runs.append(phase)
        if not phase.ok:
            result.status = "failed"
            result.molt_failure = phase.molt_failure
            result.reason = (
                f"run {run_idx} failed{_molt_failure_reason_suffix(phase.molt_failure)}"
            )
            return result
        if runner.json_stdout and not dry_run:
            if phase.stdout_json_error is not None:
                result.status = "failed"
                result.reason = (
                    f"run {run_idx} JSON parse failed: {phase.stdout_json_error}"
                )
                return result
            if phase.stdout_json is None:
                result.status = "failed"
                result.reason = f"run {run_idx} did not emit JSON stdout"
                return result
            if isinstance(phase.stdout_json, dict) and phase.stdout_json.get(
                "status"
            ) not in (None, "ok"):
                result.status = "failed"
                result.reason = (
                    f"run {run_idx} emitted non-ok JSON status: "
                    f"{phase.stdout_json.get('status')!r}"
                )
                return result
            result.structured_outputs.append(phase.stdout_json)
            for metric_name, elapsed_s in _extract_structured_elapsed(
                phase.stdout_json
            ).items():
                result.structured_samples_s.setdefault(metric_name, []).append(
                    elapsed_s
                )
        result.run_samples_s.append(phase.elapsed_s)

    if result.run_samples_s:
        result.run_median_s = statistics.median(result.run_samples_s)
        result.run_mean_s = statistics.mean(result.run_samples_s)
        if len(result.run_samples_s) > 1:
            result.run_stdev_s = statistics.stdev(result.run_samples_s)
        else:
            result.run_stdev_s = 0.0
    for metric_name, samples in result.structured_samples_s.items():
        if samples:
            result.structured_median_s[metric_name] = statistics.median(samples)
    return result


def _suite_metrics(runners: dict[str, RunnerResult]) -> dict[str, float | None]:
    def _runner_median(name: str) -> float | None:
        runner = runners.get(name)
        if runner and runner.status == "ok" and runner.role == "workload":
            return runner.run_median_s
        return None

    def _speedup(baseline_s: float | None, candidate_s: float | None) -> float | None:
        if (
            baseline_s is None
            or candidate_s is None
            or baseline_s <= 0.0
            or candidate_s <= 0.0
        ):
            return None
        return baseline_s / candidate_s

    cp_s = _runner_median("cpython")
    pp_s = _runner_median("pypy")
    mt_s = _runner_median("molt")
    codon_s = _runner_median("codon")
    friend_s = _runner_median("friend")
    nuitka_s = _runner_median("nuitka")
    pyodide_s = _runner_median("pyodide")
    tinygrad_s = _runner_median("tinygrad")
    numpy_s = _runner_median("numpy")

    # Standardized lane keys align with tools/bench.py JSON naming.
    metrics: dict[str, float | None] = {
        "cpython_median_s": cp_s,
        "pypy_median_s": pp_s,
        "molt_median_s": mt_s,
        "codon_median_s": codon_s,
        "friend_median_s": friend_s,
        "nuitka_median_s": nuitka_s,
        "pyodide_median_s": pyodide_s,
        "tinygrad_median_s": tinygrad_s,
        "numpy_median_s": numpy_s,
        "cpython_time_s": cp_s,
        "pypy_time_s": pp_s,
        "molt_time_s": mt_s,
        "codon_time_s": codon_s,
        "nuitka_time_s": nuitka_s,
        "pyodide_time_s": pyodide_s,
        "tinygrad_time_s": tinygrad_s,
        "numpy_time_s": numpy_s,
        "molt_vs_cpython_speedup": _speedup(cp_s, mt_s),
        "molt_vs_pypy_speedup": _speedup(pp_s, mt_s),
        "molt_vs_codon_speedup": _speedup(codon_s, mt_s),
        "molt_vs_friend_speedup": _speedup(friend_s, mt_s),
        "friend_vs_molt_speedup": _speedup(mt_s, friend_s),
        "molt_vs_nuitka_speedup": _speedup(nuitka_s, mt_s),
        "nuitka_vs_molt_speedup": _speedup(mt_s, nuitka_s),
        "molt_vs_pyodide_speedup": _speedup(pyodide_s, mt_s),
        "pyodide_vs_molt_speedup": _speedup(mt_s, pyodide_s),
        "molt_vs_tinygrad_speedup": _speedup(tinygrad_s, mt_s),
        "tinygrad_vs_molt_speedup": _speedup(mt_s, tinygrad_s),
        "molt_vs_numpy_speedup": _speedup(numpy_s, mt_s),
        "numpy_vs_molt_speedup": _speedup(mt_s, numpy_s),
        "molt_speedup": _speedup(cp_s, mt_s),
        "molt_cpython_ratio": _speedup(mt_s, cp_s),
        "molt_pypy_ratio": _speedup(mt_s, pp_s),
        "molt_codon_ratio": _speedup(mt_s, codon_s),
        "molt_nuitka_ratio": _speedup(mt_s, nuitka_s),
        "molt_pyodide_ratio": _speedup(mt_s, pyodide_s),
        "molt_tinygrad_ratio": _speedup(mt_s, tinygrad_s),
        "molt_numpy_ratio": _speedup(mt_s, numpy_s),
    }
    structured_by_metric: dict[str, dict[str, float]] = {}
    for runner_name, runner in runners.items():
        if runner.status != "ok" or runner.role != "workload":
            continue
        runner_slug = _metric_slug(runner_name)
        metrics[f"{runner_slug}_median_s"] = runner.run_median_s
        metrics[f"{runner_slug}_time_s"] = runner.run_median_s
        for metric_name, median_s in runner.structured_median_s.items():
            metric_slug = _metric_slug(metric_name)
            metrics[f"{runner_slug}_{metric_slug}_median_s"] = median_s
            structured_by_metric.setdefault(metric_slug, {})[runner_name] = median_s

    for metric_slug, by_runner in structured_by_metric.items():
        molt_metric_s = by_runner.get("molt")
        cpython_metric_s = by_runner.get("cpython")
        friend_metric_s = by_runner.get("friend")
        tinygrad_metric_s = by_runner.get("tinygrad")
        numpy_metric_s = by_runner.get("numpy")
        metrics[f"molt_vs_cpython_{metric_slug}_speedup"] = _speedup(
            cpython_metric_s, molt_metric_s
        )
        metrics[f"molt_vs_friend_{metric_slug}_speedup"] = _speedup(
            friend_metric_s, molt_metric_s
        )
        metrics[f"molt_vs_tinygrad_{metric_slug}_speedup"] = _speedup(
            tinygrad_metric_s, molt_metric_s
        )
        metrics[f"molt_vs_numpy_{metric_slug}_speedup"] = _speedup(
            numpy_metric_s, molt_metric_s
        )
    return metrics


def _suite_status(runners: dict[str, RunnerResult]) -> tuple[str, str | None]:
    failed = [name for name, runner in runners.items() if runner.status == "failed"]
    if failed:
        return "failed", f"runner failures: {', '.join(sorted(failed))}"
    ok_count = sum(1 for runner in runners.values() if runner.status == "ok")
    if ok_count == 0:
        return "skipped", "no runnable runners"
    return "ok", None
