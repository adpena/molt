#!/usr/bin/env python3
"""Build and run the deterministic native exact-container attestation.

The Rust child owns calibrated ns/op and allocator observation. This runner
reuses the L7 affinity, build-fingerprint, host-calibration, and guarded-build
policy; executes seven independent release processes by default; verifies every
hard steady-state zero-allocation gate and both construction positive controls;
and writes one machine-readable JSON bundle with process-tree RSS and Job commit.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import secrets
import sys
from dataclasses import asdict
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[2]
BENCH_ROOT = Path(__file__).resolve().parent
TOOLS_ROOT = REPO_ROOT / "tools"
if str(BENCH_ROOT) not in sys.path:
    sys.path.insert(0, str(BENCH_ROOT))
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import run_l7_numeric_attestation as l7  # noqa: E402
from memory_guard_core.paths import active_guard_marker_dir  # noqa: E402

DEFAULT_OUTPUT = (
    REPO_ROOT
    / "logs"
    / "benchmarks"
    / "sequence_container_attestation"
    / "latest.json"
)
CAPSULE_ACTIVE_DIR = active_guard_marker_dir(REPO_ROOT)
CAPSULE_ARCHIVE_DIR = (
    REPO_ROOT
    / "logs"
    / "benchmarks"
    / "sequence_container_attestation"
    / "custody"
)
PREFIX = "SEQUENCE_CONTAINER_ATTESTATION="
CASE_NAMES = (
    "list.delta.append_pop",
    "list.delta.indexed_replace",
    "list.delta.reverse",
    "list.construction.pylist_new_presized",
    "tuple.steady.empty_singleton",
    "tuple.steady.checked_raw_fast_items",
    "tuple.steady.full_slice_repeat_one_identity",
    "tuple.construction.pytuple_new_fill",
)
ZERO_ALLOCATION_CASES = frozenset(
    {
        "list.delta.append_pop",
        "list.delta.indexed_replace",
        "list.delta.reverse",
        "tuple.steady.empty_singleton",
        "tuple.steady.checked_raw_fast_items",
        "tuple.steady.full_slice_repeat_one_identity",
    }
)
POSITIVE_CONTROL_CASES = frozenset(CASE_NAMES) - ZERO_ALLOCATION_CASES
METRICS = (
    "ns_per_op",
    "allocations_per_op",
    "allocated_bytes_per_op",
    "peak_live_bytes",
)
MAX_TIMING_ROBUST_CV = 0.20
MAX_PROCESS_MEMORY_ROBUST_CV = 0.20
MAX_PEAK_RSS_BYTES = 2 * 1024 * 1024 * 1024
MAX_PEAK_JOB_COMMIT_BYTES = 2 * 1024 * 1024 * 1024
CONFIG = {
    "package": "molt-runtime",
    "test": "list_delta_perf_attestation",
    "features": ["l7-attestation-probe"],
    "timing_instrumentation": (
        "test-feature probe leaves one relaxed TRACK atomic load in allocator "
        "calls during timed loops; allocation counters are disabled"
    ),
}


def _finite_nonnegative(value: Any, context: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise RuntimeError(f"{context} must be numeric")
    result = float(value)
    if not math.isfinite(result) or result < 0.0:
        raise RuntimeError(f"{context} must be finite and nonnegative")
    return result


def _parse_payload(stdout: str) -> dict[str, Any]:
    lines = [line.split(PREFIX, 1)[1] for line in stdout.splitlines() if PREFIX in line]
    if len(lines) != 1:
        raise RuntimeError(
            f"expected exactly one container attestation record, found {len(lines)}"
        )
    payload = json.loads(
        lines[0],
        parse_constant=lambda token: (_ for _ in ()).throw(
            ValueError(f"non-finite JSON constant {token!r} is forbidden")
        ),
    )
    if not isinstance(payload, dict):
        raise RuntimeError("container attestation payload must be an object")
    return payload


def _validate_payload(
    payload: dict[str, Any],
    *,
    source: dict[str, Any],
    build: dict[str, Any],
    rustc: str,
    run_nonce: str,
    execution_control: dict[str, Any],
) -> None:
    expected_source = {
        "git_commit": source["git_commit"],
        "git_dirty": source["git_dirty"],
        "rustc": rustc,
        "build_fingerprint": build["artifact_fingerprint"],
        "run_nonce": run_nonce,
    }
    if payload.get("schema_version") != 2:
        raise RuntimeError("container attestation schema version drift")
    if payload.get("kind") != "sequence_container_performance_attestation":
        raise RuntimeError("container attestation kind drift")
    if payload.get("profile") != "release":
        raise RuntimeError("container attestation was not built in release mode")
    if payload.get("source") != expected_source:
        raise RuntimeError("container attestation source provenance echo mismatch")
    if payload.get("execution_control") != l7._child_execution_control(execution_control):
        raise RuntimeError("container attestation affinity control drift")
    expected_mode = {
        "deterministic_default": True,
        "runtime_gil": "enabled",
        "free_threaded": False,
        "benchmark_threads": 1,
    }
    if payload.get("execution_mode") != expected_mode:
        raise RuntimeError("container attestation escaped deterministic GIL-default mode")
    cases = payload.get("cases")
    if not isinstance(cases, list) or [case.get("name") for case in cases] != list(CASE_NAMES):
        raise RuntimeError("container attestation ordered case manifest drift")
    sample_count = payload.get("sample_count")
    if sample_count != l7.SAMPLE_COUNT:
        raise RuntimeError("container attestation sample-count drift")

    for case in cases:
        name = case["name"]
        samples = case.get("samples")
        if not isinstance(samples, list) or len(samples) != sample_count:
            raise RuntimeError(f"{name}: sample count mismatch")
        if case.get("gates", {}).get("semantic_witness") != "pass":
            raise RuntimeError(f"{name}: semantic witness did not pass")
        zero_gate = case.get("gates", {}).get("steady_state_zero_allocations")
        if not isinstance(zero_gate, dict) or zero_gate.get("passed") is not True:
            raise RuntimeError(f"{name}: allocation gate did not pass")
        required = name in ZERO_ALLOCATION_CASES
        if zero_gate.get("required") is not required:
            raise RuntimeError(f"{name}: zero-allocation gate scope drift")
        positive_gate = case.get("gates", {}).get("allocator_probe_positive_control")
        if not isinstance(positive_gate, dict) or positive_gate.get("passed") is not True:
            raise RuntimeError(f"{name}: allocator positive-control gate did not pass")
        if positive_gate.get("required") is required:
            raise RuntimeError(f"{name}: allocator positive-control scope drift")
        if positive_gate.get("required") is not (name in POSITIVE_CONTROL_CASES):
            raise RuntimeError(f"{name}: allocator positive-control manifest drift")
        for sample_index, sample in enumerate(samples):
            for metric in METRICS:
                value = _finite_nonnegative(
                    sample.get(metric), f"{name}/sample-{sample_index}/{metric}"
                )
                if required and metric != "ns_per_op" and value != 0.0:
                    raise RuntimeError(
                        f"{name}/sample-{sample_index}: {metric} violated exact zero gate"
                    )
        summary = case.get("summary")
        if not isinstance(summary, dict):
            raise RuntimeError(f"{name}: summary missing")
        for metric in METRICS:
            metric_summary = summary.get(metric)
            if not isinstance(metric_summary, dict):
                raise RuntimeError(f"{name}: {metric} summary missing")
            for field in ("median", "cv", "robust_cv"):
                _finite_nonnegative(metric_summary.get(field), f"{name}/{metric}/{field}")
        timing_robust_cv = _finite_nonnegative(
            summary["ns_per_op"]["robust_cv"], f"{name}/ns_per_op/robust_cv"
        )
        if timing_robust_cv > MAX_TIMING_ROBUST_CV:
            raise RuntimeError(
                f"{name}: timing robust CV {timing_robust_cv:.4f} exceeds "
                f"{MAX_TIMING_ROBUST_CV:.4f}"
            )


def _capsule_paths(run_nonce: str, run_index: int) -> tuple[Path, Path]:
    name = f"sequence-container-{run_nonce}-run-{run_index:02d}.json"
    return CAPSULE_ACTIVE_DIR / name, CAPSULE_ARCHIVE_DIR / name


def _aggregate(attestations: list[dict[str, Any]]) -> dict[str, Any]:
    aggregate: dict[str, Any] = {}
    for case_name in CASE_NAMES:
        cases = [
            next(case for case in payload["cases"] if case["name"] == case_name)
            for payload in attestations
        ]
        metrics = {
            metric: l7._summary(
                [_finite_nonnegative(case["summary"][metric]["median"], metric) for case in cases]
            )
            for metric in METRICS
        }
        aggregate[case_name] = {
            "family": cases[0]["family"],
            "input": cases[0]["input"],
            "metrics": metrics,
            "steady_state_zero_allocations": cases[0]["gates"][
                "steady_state_zero_allocations"
            ]["required"],
        }
    return aggregate


def run_attestation(
    *,
    runs: int,
    timeout: float,
    output: Path,
    affinity_request: str,
) -> dict[str, Any]:
    if not 7 <= runs <= 9:
        raise ValueError("--runs must be between 7 and 9")
    if not math.isfinite(timeout) or timeout <= 0.0:
        raise ValueError("--timeout must be finite and positive")
    execution_control = l7._resolve_execution_control(affinity_request)
    source_start = l7._source_snapshot()
    rustc = l7._parent_command(["rustc", "--version", "--verbose"]).decode().strip()
    cargo_lock_sha256 = l7._sha256_file(REPO_ROOT / "Cargo.lock")
    build_executable, build = l7._build_test_executable(
        CONFIG,
        timeout=timeout,
        source=source_start,
        rustc=rustc,
        cargo_lock_sha256=cargo_lock_sha256,
    )
    run_nonce = secrets.token_hex(16)
    argv = [
        str(build_executable),
        "sequence_container_performance_attestation",
        "--exact",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]
    child_env = {
        "MOLT_L7_GIT_COMMIT": source_start["git_commit"],
        "MOLT_L7_GIT_DIRTY": "true" if source_start["git_dirty"] else "false",
        "MOLT_L7_RUSTC": rustc,
        "MOLT_L7_BUILD_FINGERPRINT": build["artifact_fingerprint"],
        "MOLT_L7_AFFINITY_MASK": execution_control["affinity_mask"],
        "MOLT_L7_RUN_NONCE": run_nonce,
    }
    process_runs: list[dict[str, Any]] = []
    attestations: list[dict[str, Any]] = []
    for run_index in range(1, runs + 1):
        before = asdict(l7.perf_calibration.measure_quiescence())
        active_capsule, archived_capsule = _capsule_paths(run_nonce, run_index)
        capsule = {
            "kind": "sequence_container_attestation_death_capsule",
            "command": argv,
            "cwd": str(REPO_ROOT),
            "guard_pid": os.getpid(),
            "child_pid": None,
            "run": run_index,
            "run_nonce": run_nonce,
            "status": "starting",
            "timestamp": l7._utc_now(),
            "evidence_path": str(archived_capsule),
            "quiescence_before": before,
        }
        l7._write_json_atomic(active_capsule, capsule)
        try:
            def record_child_pid(pid: int) -> None:
                capsule.update(
                    {
                        "child_pid": pid,
                        "status": "running",
                        "child_started_at_utc": l7._utc_now(),
                    }
                )
                l7._write_json_atomic(active_capsule, capsule)

            measured = l7.perf_calibration.run_and_measure(
                argv,
                timeout=timeout,
                cwd=str(REPO_ROOT),
                env=child_env,
                on_spawn=record_child_pid,
            )
            after = asdict(l7.perf_calibration.measure_quiescence())
            capsule.update(
                {
                    "status": (
                        "completed"
                        if measured.returncode == 0 and not measured.timed_out
                        else "failed"
                    ),
                    "completed_at_utc": l7._utc_now(),
                    "returncode": measured.returncode,
                    "timed_out": measured.timed_out,
                    "peak_rss_bytes": measured.peak_rss_bytes,
                    "peak_job_commit_bytes": measured.peak_job_commit_bytes,
                    "quiescence_after": after,
                }
            )
        except BaseException as exc:
            capsule.update(
                {
                    "status": "runner_error",
                    "completed_at_utc": l7._utc_now(),
                    "error": f"{type(exc).__name__}: {exc}",
                }
            )
            l7._write_json_atomic(active_capsule, capsule)
            archived_capsule.parent.mkdir(parents=True, exist_ok=True)
            active_capsule.replace(archived_capsule)
            raise
        l7._write_json_atomic(active_capsule, capsule)
        archived_capsule.parent.mkdir(parents=True, exist_ok=True)
        active_capsule.replace(archived_capsule)

        if measured.returncode != 0 or measured.timed_out:
            sys.stderr.write(measured.stdout)
            sys.stderr.write(measured.stderr)
            raise RuntimeError(
                f"container attestation run {run_index}/{runs} failed: "
                f"rc={measured.returncode} timeout={measured.timed_out}"
            )
        if measured.peak_rss_bytes is None:
            raise RuntimeError("container attestation process-tree peak RSS was unavailable")
        if measured.peak_rss_bytes > MAX_PEAK_RSS_BYTES:
            raise RuntimeError(
                f"run {run_index} peak RSS {measured.peak_rss_bytes} exceeds "
                f"{MAX_PEAK_RSS_BYTES}"
            )
        if sys.platform == "win32" and measured.peak_job_commit_bytes is None:
            raise RuntimeError("container attestation Windows Job peak commit was unavailable")
        if (
            measured.peak_job_commit_bytes is not None
            and measured.peak_job_commit_bytes > MAX_PEAK_JOB_COMMIT_BYTES
        ):
            raise RuntimeError(
                f"run {run_index} Job peak commit {measured.peak_job_commit_bytes} exceeds "
                f"{MAX_PEAK_JOB_COMMIT_BYTES}"
            )
        if not l7._quiescence_ok(before) or not l7._quiescence_ok(after):
            raise RuntimeError(f"run {run_index} was not quiescent")
        payload = _parse_payload(measured.stdout)
        _validate_payload(
            payload,
            source=source_start,
            build=build,
            rustc=rustc,
            run_nonce=run_nonce,
            execution_control=execution_control,
        )
        process_runs.append(
            {
                "run": run_index,
                "elapsed_ms": measured.elapsed_s * 1000.0,
                "peak_rss_bytes": measured.peak_rss_bytes,
                "peak_job_commit_bytes": measured.peak_job_commit_bytes,
                "stdout_sha256": l7._sha256_bytes(measured.stdout.encode()),
                "stderr_sha256": l7._sha256_bytes(measured.stderr.encode()),
                "quiescence_before": before,
                "quiescence_after": after,
                "custody": str(archived_capsule),
            }
        )
        attestations.append(payload)

    source_end = l7._source_snapshot()
    if source_end != source_start:
        raise RuntimeError("repository source changed during container attestation")
    fingerprint = l7.perf_calibration.host_fingerprint()
    fingerprint_data = asdict(fingerprint)
    fingerprint_data["key"] = fingerprint.key()
    aggregate = _aggregate(attestations)
    for case_name, case in aggregate.items():
        robust_cv = _finite_nonnegative(
            case["metrics"]["ns_per_op"]["robust_cv"],
            f"{case_name}/cross_process/ns_per_op/robust_cv",
        )
        if robust_cv > MAX_TIMING_ROBUST_CV:
            raise RuntimeError(
                f"{case_name}: cross-process timing robust CV {robust_cv:.4f} exceeds "
                f"{MAX_TIMING_ROBUST_CV:.4f}"
            )
    rss_summary = l7._summary(
        [float(row["peak_rss_bytes"]) for row in process_runs]
    )
    rss_robust_cv = _finite_nonnegative(
        rss_summary["robust_cv"], "process/peak_rss_bytes/robust_cv"
    )
    if rss_robust_cv > MAX_PROCESS_MEMORY_ROBUST_CV:
        raise RuntimeError(
            f"process peak RSS robust CV {rss_robust_cv:.4f} exceeds "
            f"{MAX_PROCESS_MEMORY_ROBUST_CV:.4f}"
        )
    job_commit_values = [
        float(row["peak_job_commit_bytes"])
        for row in process_runs
        if row["peak_job_commit_bytes"] is not None
    ]
    job_commit_summary = l7._summary(job_commit_values) if job_commit_values else None
    if job_commit_summary is not None:
        job_commit_robust_cv = _finite_nonnegative(
            job_commit_summary["robust_cv"],
            "process/peak_job_commit_bytes/robust_cv",
        )
        if job_commit_robust_cv > MAX_PROCESS_MEMORY_ROBUST_CV:
            raise RuntimeError(
                f"Job peak commit robust CV {job_commit_robust_cv:.4f} exceeds "
                f"{MAX_PROCESS_MEMORY_ROBUST_CV:.4f}"
            )
    result = {
        "schema_version": 2,
        "kind": "sequence_container_performance_attestation_bundle",
        "generated_at_utc": l7._utc_now(),
        "validation": {
            "valid": True,
            "semantic_witnesses": "all_passed",
            "steady_state_zero_allocation_gates": "all_passed",
            "allocator_probe_positive_control": "passed",
            "timing_stability": "passed",
            "memory_stability_and_budget": "passed",
        },
        "runner": {
            "path": str(Path(__file__).resolve().relative_to(REPO_ROOT)),
            "runs": runs,
            "execution_control": execution_control,
            "timing_scope": "loop_inclusive; allocation observer is untimed",
            "mode": "native release, deterministic runtime GIL, one benchmark thread",
            "limits": {
                "max_timing_robust_cv": MAX_TIMING_ROBUST_CV,
                "max_process_memory_robust_cv": MAX_PROCESS_MEMORY_ROBUST_CV,
                "max_peak_rss_bytes": MAX_PEAK_RSS_BYTES,
                "max_peak_job_commit_bytes": MAX_PEAK_JOB_COMMIT_BYTES,
            },
        },
        "source": {
            "start": source_start,
            "end": source_end,
            "rustc": rustc,
            "cargo_lock_sha256": cargo_lock_sha256,
            "runner_sha256": l7._sha256_file(Path(__file__).resolve()),
            "run_nonce": run_nonce,
        },
        "host": {"fingerprint": fingerprint_data},
        "build": build,
        "process": {
            "elapsed_ms": l7._summary([row["elapsed_ms"] for row in process_runs]),
            "memory_scope": "Windows Job process tree when available; root-process fallback elsewhere",
            "peak_rss_bytes": rss_summary,
            "peak_job_commit_bytes": job_commit_summary,
            "peak_job_commit_available": job_commit_summary is not None,
            "runs": process_runs,
        },
        "aggregated_cases": aggregate,
        "attestations": attestations,
    }
    l7._write_json_atomic(output, result)
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=7, choices=range(7, 10))
    parser.add_argument("--timeout", type=float, default=300.0)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument(
        "--affinity-mask",
        default="auto",
        help="single-logical-CPU mask, or auto to reuse the L7 selection policy",
    )
    args = parser.parse_args(argv)
    output = args.output if args.output.is_absolute() else REPO_ROOT / args.output
    try:
        run_attestation(
            runs=args.runs,
            timeout=args.timeout,
            output=output,
            affinity_request=args.affinity_mask,
        )
    except ValueError as exc:
        parser.error(str(exc))
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
