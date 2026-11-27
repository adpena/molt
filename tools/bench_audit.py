"""Benchmark audit tool for Project TITAN Phase 0.

Runs each benchmark through both Molt (native) and CPython, computes speedup,
and classifies results as Green/Yellow/Red.

Usage:
    uv run python tools/bench_audit.py [--samples N] [--timeout S] [--out FILE]
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import statistics
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, Optional

# ---------------------------------------------------------------------------
# Re-use infrastructure from bench.py where possible
# ---------------------------------------------------------------------------

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

REPO_ROOT = Path(__file__).parent.parent.resolve()

from tools import bench as _bench  # noqa: E402
from tools import bench_suites  # noqa: E402
from tools import harness_memory_guard  # noqa: E402
from tools import perf_authority  # noqa: E402
from tools.bench_evidence import (  # noqa: E402
    AdmittedBenchmarkWorkload,
    BenchmarkWorkloadRecord,
    admit_benchmark_workload,
)
from tools.bench_metadata import benchmark_reference_contract  # noqa: E402
from molt.process_environment import ProcessEnvironment  # noqa: E402
from molt.python_environment_identity import PythonIdentityVerifier  # noqa: E402
from molt.python_interpreter import (  # noqa: E402
    BenchmarkPythonMetadata,
    PythonInterpreter,
    benchmark_python_metadata,
    resolve_benchmark_python,
)

prepare_molt_binary = _bench.prepare_molt_binary
measure_molt_run = _bench.measure_molt_run

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

DEFAULT_SAMPLES = 5
DEFAULT_TIMEOUT_S = 60.0

# Speedup thresholds
GREEN_THRESHOLD = 1.0  # Molt is at least as fast as CPython
YELLOW_THRESHOLD = 0.5  # Molt is within 2x of CPython

RESULTS_DIR = REPO_ROOT / "bench" / "results"
DEFAULT_OUTPUT = RESULTS_DIR / "audit_baseline.json"


# ---------------------------------------------------------------------------
# Data classes
# ---------------------------------------------------------------------------


AuditClassification = Literal["Green", "Yellow", "Red", "Error", "Skipped", "Molt-only"]


@dataclass
class BenchResult:
    name: str
    workload: BenchmarkWorkloadRecord
    molt_s: Optional[float]
    cpython_s: Optional[float]
    speedup: Optional[float]
    classification: AuditClassification
    error: Optional[str] = None
    molt_build_s: Optional[float] = None
    perf_data: dict[str, int] | None = None
    cpython_oracle: BenchmarkPythonMetadata | None = None
    molt_output_parity: dict[str, object] | None = None


# ---------------------------------------------------------------------------
# Timing helpers
# ---------------------------------------------------------------------------


def _cpython_samples(
    workload: AdmittedBenchmarkWorkload,
    samples: int,
    timeout_s: float,
    baseline: PythonInterpreter,
    environment: ProcessEnvironment,
) -> _bench.SampleBatch:
    """Capture complete CPython samples from the admitted reference invocation."""
    invocation = workload.reference_invocation(baseline.command, environment)
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH", invocation.environment)
    batch = _bench.collect_samples(
        lambda: _bench.measure_runtime(
            list(invocation.command),
            env=invocation.environment,
            cwd=invocation.cwd,
            timeout_s=timeout_s,
            label=f"cpython:{workload.script.name}",
            limits=limits,
        ),
        samples,
        validate_inputs=workload.verify,
    )
    if batch.ok:
        for sample in (*batch.warmup_samples, *batch.samples):
            try:
                workload.verify_reference_output(sample.stdout, sample.stderr)
            except ValueError as exc:
                return _bench.SampleBatch(
                    batch.samples,
                    False,
                    batch.warmup_samples,
                    _bench.MoltFailure(
                        phase="reference",
                        status="output_mismatch",
                        returncode=0,
                        timed_out=False,
                        elapsed_s=sample.elapsed_s,
                        detail="sealed_oracle_mismatch",
                        message=str(exc),
                        stdout=sample.stdout,
                        stderr=sample.stderr,
                    ),
                )
    return batch


def _prepare_workload(
    workload: AdmittedBenchmarkWorkload, environment: ProcessEnvironment
) -> _bench.MoltBinary | _bench.MoltFailure:
    workload.verify()
    return prepare_molt_binary(
        str(workload.execution_script),
        extra_args=workload.record["molt_args"],
        env=workload.compiler_environment(environment),
        source_cwd=workload.source_cwd,
    )


def _molt_samples(
    workload: AdmittedBenchmarkWorkload,
    samples: int,
    timeout_s: float,
    environment: ProcessEnvironment,
) -> tuple[_bench.SampleBatch, float | None]:
    """Build and sample the admitted Molt workload, then close its scratch owner."""
    binary = _prepare_workload(workload, environment)
    if isinstance(binary, _bench.MoltFailure):
        workload.verify()
        return _bench.SampleBatch([], False, failure=binary), None
    primary: BaseException | None = None
    try:
        invocation = workload.guest_invocation([str(binary.path)], environment)
        limits = harness_memory_guard.limits_from_env(
            "MOLT_BENCH", invocation.environment
        )
        batch = _bench.collect_samples(
            lambda: measure_molt_run(
                binary.path,
                env=invocation.environment,
                cwd=invocation.cwd,
                run_args=list(invocation.command[1:]),
                label=f"molt:{workload.script.name}",
                timeout_s=timeout_s,
                limits=limits,
            ),
            samples,
            validate_inputs=workload.verify,
        )
        return batch, binary.build_s
    except BaseException as exc:
        primary = exc
        raise
    finally:
        _bench._cleanup_bench_temp(binary.temp_dir, primary)


# ---------------------------------------------------------------------------
# perf counter collection (Linux only)
# ---------------------------------------------------------------------------


def _collect_perf_data(
    binary_path: Path,
    *,
    workload: AdmittedBenchmarkWorkload,
    environment: ProcessEnvironment,
) -> dict[str, int] | None:
    """Collect instruction count and cache-miss data via `perf stat`."""
    if platform.system() != "Linux":
        return None
    perf = shutil.which("perf")
    if perf is None:
        return None
    events = "instructions,cache-misses,cache-references"
    guest = workload.guest_invocation([str(binary_path)], environment)
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH", guest.environment)
    try:
        res = harness_memory_guard.guarded_completed_process(
            [perf, "stat", "-e", events, "--", *guest.command],
            prefix="MOLT_BENCH",
            cwd=guest.cwd,
            env=guest.environment,
            capture_output=True,
            text=True,
            timeout=30.0,
            limits=limits,
        )
    except Exception:
        return None
    if res.returncode != 0:
        return None
    # Parse the perf stat stderr output
    data: dict[str, int] = {}
    for line in res.stderr.splitlines():
        line = line.strip()
        for key, token in [
            ("instructions", "instructions"),
            ("cache_misses", "cache-misses"),
            ("cache_references", "cache-references"),
        ]:
            if token in line:
                parts = line.split()
                if parts:
                    try:
                        data[key] = int(parts[0].replace(",", "").replace(".", ""))
                    except ValueError:
                        pass
    return data or None


def _collect_perf_rebuild(
    workload: AdmittedBenchmarkWorkload, environment: ProcessEnvironment
) -> dict[str, int] | None:
    binary = _prepare_workload(workload, environment)
    if isinstance(binary, _bench.MoltFailure):
        workload.verify()
        return None
    primary: BaseException | None = None
    try:
        workload.verify()
        perf = _collect_perf_data(
            binary.path, workload=workload, environment=environment
        )
        workload.verify()
        return perf
    except BaseException as exc:
        primary = exc
        raise
    finally:
        _bench._cleanup_bench_temp(binary.temp_dir, primary)


# ---------------------------------------------------------------------------
# Classification
# ---------------------------------------------------------------------------


def _classify(speedup: Optional[float]) -> AuditClassification:
    if speedup is None:
        return "Error"
    if speedup >= GREEN_THRESHOLD:
        return "Green"
    if speedup >= YELLOW_THRESHOLD:
        return "Yellow"
    return "Red"


# ---------------------------------------------------------------------------
# Core audit loop
# ---------------------------------------------------------------------------


def audit_benchmarks(
    benchmarks: list[str],
    samples: int = DEFAULT_SAMPLES,
    timeout_s: float = DEFAULT_TIMEOUT_S,
    verbose: bool = False,
    skip_molt: bool = False,
) -> list[BenchResult]:
    if samples <= 0:
        raise ValueError("benchmark audit requires positive sample count")
    results: list[BenchResult] = []
    environment = ProcessEnvironment(os.environ)
    baseline = None
    cpython_oracle = None
    if any(
        benchmark_reference_contract(path).external_baselines for path in benchmarks
    ):
        with PythonIdentityVerifier() as verifier:
            baseline = resolve_benchmark_python(
                env=environment, cwd=REPO_ROOT, verifier=verifier
            )
        cpython_oracle = benchmark_python_metadata(baseline)
        print(f"CPython: {baseline.version} ({baseline.executable})")

    for i, bench_path in enumerate(benchmarks, 1):
        # Resolve relative to repo root
        script = REPO_ROOT / bench_path
        name = script.stem
        contract = benchmark_reference_contract(str(script))
        has_reference = contract.external_baselines

        print(f"[{i:2d}/{len(benchmarks)}] {name} ...", flush=True)

        with admit_benchmark_workload(
            script,
            project_root=REPO_ROOT,
            run_args=bench_suites.application_run_args(script),
            molt_args=bench_suites.molt_args_for_benchmark(script),
            cpython_command=baseline.command if has_reference and baseline else None,
        ) as workload:
            reference_batch = (
                _cpython_samples(workload, samples, timeout_s, baseline, environment)
                if has_reference and baseline is not None
                else None
            )
            reference_ready = (
                reference_batch is not None
                and _bench._stable_output(reference_batch) is not None
            )
            if skip_molt:
                molt_batch, build_s = None, None
            elif has_reference and not reference_ready:
                molt_batch, build_s = _bench.SampleBatch([], False), None
            else:
                molt_batch, build_s = _molt_samples(
                    workload, samples, timeout_s, environment
                )
            cpython_s = (
                statistics.median(reference_batch.times_s)
                if reference_batch is not None and reference_batch.ok
                else None
            )
            molt_s = (
                statistics.median(molt_batch.times_s)
                if molt_batch is not None and molt_batch.ok
                else None
            )
            problems = []
            for runtime, batch in (("CPython", reference_batch), ("Molt", molt_batch)):
                if batch is not None and batch.failure is not None:
                    failure = batch.failure
                    detail = failure.detail or failure.message or failure.status
                    problems.append(f"{runtime} {detail}")

            parity = None
            if molt_batch is not None and has_reference:
                parity = _bench._output_parity_evidence(
                    reference_batch,
                    molt_batch,
                    reference_runtime=contract.reference_runtime,
                    reference_required=has_reference,
                    reference_reason=contract.reason,
                )
                if parity["ok"] is False:
                    molt_s = None
                    reason = parity["reason"]
                    if reason != "molt_unavailable":
                        problems.append(str(reason))
                    if reason == "reference_unstable":
                        cpython_s = None
            elif molt_batch is not None and molt_batch.ok:
                output = _bench._stable_output(molt_batch)
                if output is None or not contract.molt_output_matches(*output):
                    molt_s = None
                    problems.append("intrinsic_oracle_mismatch")
            elif (
                reference_batch is not None
                and reference_batch.ok
                and not reference_ready
            ):
                cpython_s = None
                problems.append("reference_unstable")

            speedup = (
                perf_authority.signed_ratio_value(
                    cpython_s, molt_s, direction=perf_authority.RatioDirection.SPEEDUP
                )
                if parity is not None and parity["ok"] is True
                else None
            )
            perf_data = None
            if (
                molt_s is not None
                and (not has_reference or speedup is not None)
                and platform.system() == "Linux"
                and shutil.which("perf")
            ):
                perf_data = _collect_perf_rebuild(workload, environment)
            elif verbose and platform.system() == "Darwin":
                print(
                    "  [note] macOS: perf counters require Instruments (not automated)"
                )
            workload_record = workload.record

        classification: AuditClassification
        if not has_reference and molt_s is not None:
            classification = "Molt-only"
        elif skip_molt and (cpython_s is not None or not has_reference):
            classification = "Skipped"
        else:
            classification = _classify(speedup)
        color_tag = {"Green": "\033[32m", "Yellow": "\033[33m", "Red": "\033[31m"}.get(
            classification, "\033[90m"
        )
        reset = "\033[0m"

        if speedup is not None:
            print(
                f"  CPython: {cpython_s:.4f}s  Molt: {molt_s:.4f}s  "
                f"Speedup: {speedup:.2f}x  {color_tag}{classification}{reset}"
            )
        elif skip_molt and cpython_s is not None:
            print(f"  CPython: {cpython_s:.4f}s  (Molt skipped)")
        elif not has_reference and molt_s is not None:
            print(f"  Molt: {molt_s:.4f}s  (Molt-only)")
        else:
            detail = ", ".join(problems) if problems else "no qualified comparison"
            print(f"  {color_tag}{classification}{reset}: {detail}")

        results.append(
            BenchResult(
                name=name,
                workload=workload_record,
                molt_s=molt_s,
                cpython_s=cpython_s,
                speedup=speedup,
                classification=classification,
                error=", ".join(problems) if problems else None,
                molt_build_s=build_s,
                perf_data=perf_data,
                cpython_oracle=cpython_oracle if has_reference else None,
                molt_output_parity=parity,
            )
        )

    return results


# ---------------------------------------------------------------------------
# Output / summary
# ---------------------------------------------------------------------------


def _to_json_record(r: BenchResult) -> dict:
    record: dict = {
        "name": r.name,
        "workload": r.workload,
        "molt_s": round(r.molt_s, 6) if r.molt_s is not None else None,
        "cpython_s": round(r.cpython_s, 6) if r.cpython_s is not None else None,
        "cpython_oracle": r.cpython_oracle,
        "molt_output_parity": r.molt_output_parity,
        "speedup": round(r.speedup, 4) if r.speedup is not None else None,
        "ratio_directions": {
            "speedup": perf_authority.RatioDirection.SPEEDUP.value,
        },
        "class": r.classification,
    }
    if r.molt_build_s is not None:
        record["molt_build_s"] = round(r.molt_build_s, 4)
    if r.error is not None:
        record["error"] = r.error
    if r.perf_data:
        record["perf"] = r.perf_data
    return record


def print_summary(results: list[BenchResult]) -> None:
    green = [r for r in results if r.classification == "Green"]
    yellow = [r for r in results if r.classification == "Yellow"]
    red = [r for r in results if r.classification == "Red"]
    error = [r for r in results if r.classification == "Error"]
    skipped = [r for r in results if r.classification == "Skipped"]
    molt_only = [r for r in results if r.classification == "Molt-only"]

    print()
    print("=" * 72)
    print("BENCHMARK AUDIT SUMMARY")
    print("=" * 72)
    print(f"  Total:   {len(results)}")
    print(f"  \033[32mGreen\033[0m (>= 1x faster):  {len(green)}")
    print(f"  \033[33mYellow\033[0m (0.5x–1x):       {len(yellow)}")
    print(f"  \033[31mRed\033[0m (< 0.5x):           {len(red)}")
    print(f"  Error:               {len(error)}")
    if molt_only:
        print(f"  Molt-only:           {len(molt_only)}")
    if skipped:
        print(f"  Skipped (--cpython-only): {len(skipped)}")
    print()

    if green:
        print("Green benchmarks (Molt wins):")
        for r in sorted(green, key=lambda x: -(x.speedup or 0)):
            print(f"  {r.name:<40s} {r.speedup:.2f}x")

    if yellow:
        print()
        print("Yellow benchmarks (within 2x):")
        for r in sorted(yellow, key=lambda x: -(x.speedup or 0)):
            print(f"  {r.name:<40s} {r.speedup:.2f}x")

    if red:
        print()
        print("Red benchmarks (Molt > 2x slower):")
        for r in sorted(red, key=lambda x: x.speedup or 0):
            print(f"  {r.name:<40s} {r.speedup:.2f}x")

    if error:
        print()
        print("Errors / skipped:")
        for r in error:
            msg = r.error or "(no details)"
            print(f"  {r.name:<40s} {msg}")

    if skipped:
        print()
        print("CPython-only baseline (Molt skipped):")
        for r in skipped:
            t = f"{r.cpython_s:.4f}s" if r.cpython_s is not None else "failed"
            print(f"  {r.name:<40s} CPython: {t}")

    print("=" * 72)

    # Geometric mean of speedups for benchmarks that completed
    completed = [r for r in results if r.speedup is not None]
    if completed:
        import math

        log_sum = sum(math.log(r.speedup) for r in completed)
        geo_mean = math.exp(log_sum / len(completed))
        print(f"Geometric mean speedup (n={len(completed)}): {geo_mean:.3f}x")

    print()


def save_results(results: list[BenchResult], output_path: Path) -> None:
    output_path.parent.mkdir(parents=True, exist_ok=True)
    records = [_to_json_record(r) for r in results]
    output_path.write_text(json.dumps(records, indent=2) + "\n", encoding="utf-8")
    print(f"Results saved to {output_path}")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="Benchmark audit tool: compare Molt vs CPython on all benchmarks.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument(
        "--samples",
        type=int,
        default=DEFAULT_SAMPLES,
        metavar="N",
        help=f"Number of timing samples per benchmark (default: {DEFAULT_SAMPLES})",
    )
    p.add_argument(
        "--timeout",
        type=float,
        default=DEFAULT_TIMEOUT_S,
        metavar="SECONDS",
        help=f"Per-run timeout in seconds (default: {DEFAULT_TIMEOUT_S:.0f})",
    )
    p.add_argument(
        "--out",
        type=Path,
        default=DEFAULT_OUTPUT,
        metavar="FILE",
        help=f"JSON output path (default: {DEFAULT_OUTPUT})",
    )
    p.add_argument(
        "--filter",
        metavar="PATTERN",
        help="Only run benchmarks whose name contains PATTERN",
    )
    p.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Print extra diagnostic information",
    )
    p.add_argument(
        "--cpython-only",
        action="store_true",
        help="Skip Molt runs (useful for baseline collection without the compiler)",
    )
    p.add_argument(
        "--list",
        action="store_true",
        help="List available benchmarks and exit",
    )
    return p


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    # Build the list of benchmark paths
    benchmarks = list(bench_suites.BENCHMARKS)

    if args.filter:
        benchmarks = [b for b in benchmarks if args.filter in Path(b).stem]

    if args.list:
        print(f"Available benchmarks ({len(benchmarks)}):")
        for b in benchmarks:
            print(f"  {b}")
        return 0

    print(f"Benchmark audit: {len(benchmarks)} benchmarks, {args.samples} samples each")
    print(f"Platform: {platform.system()} {platform.machine()}")

    if platform.system() == "Darwin":
        print("Note: perf counters require Instruments on macOS (not automated here)")
    elif platform.system() == "Linux" and shutil.which("perf"):
        print("Note: perf counters will be collected via 'perf stat'")

    print()

    if args.cpython_only:
        print("--cpython-only mode: Molt runs will be skipped")

    results = audit_benchmarks(
        benchmarks,
        samples=args.samples,
        timeout_s=args.timeout,
        verbose=args.verbose,
        skip_molt=args.cpython_only,
    )

    print_summary(results)
    save_results(results, args.out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
