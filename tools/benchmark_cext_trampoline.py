#!/usr/bin/env python3
"""Fresh-process C-extension trampoline performance and allocation attestation."""

from __future__ import annotations

import argparse
from contextlib import contextmanager
import ctypes
import hashlib
import json
import math
import os
from pathlib import Path
import statistics
import sys
import time
import uuid


ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from molt.process_guard import run_completed_command  # noqa: E402


TEST_NAME = (
    "cpython_abi_hooks::tests::single_thread_extension_call_preemption_bench"
)
SAMPLE_PREFIX = "MOLT_CEXT_BENCH_SAMPLE "
DEFAULT_OUTPUT = ROOT / "logs" / "benchmarks" / "cext_trampoline" / "latest.json"
# One-sided 95% Student-t critical values, indexed by degrees of freedom.
_T95 = (
    math.inf,
    6.314,
    2.920,
    2.353,
    2.132,
    2.015,
    1.943,
    1.895,
    1.860,
    1.833,
    1.812,
    1.796,
    1.782,
    1.771,
    1.761,
    1.753,
    1.746,
    1.740,
    1.734,
    1.729,
    1.725,
    1.721,
    1.717,
    1.714,
    1.711,
    1.708,
    1.706,
    1.703,
    1.701,
    1.699,
    1.697,
)


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def executable_identity(path: Path) -> dict[str, object]:
    resolved = path.resolve(strict=True)
    stat = resolved.stat()
    return {
        "path": str(resolved),
        "size_bytes": stat.st_size,
        "sha256": _sha256(resolved),
    }


def sampling_platform_admission() -> dict[str, object]:
    if sys.platform == "win32":
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        get_process = kernel32.GetCurrentProcess
        get_process.restype = ctypes.c_void_p
        get_affinity = kernel32.GetProcessAffinityMask
        get_affinity.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_size_t),
            ctypes.POINTER(ctypes.c_size_t),
        ]
        original = ctypes.c_size_t()
        system = ctypes.c_size_t()
        if not get_affinity(get_process(), ctypes.byref(original), ctypes.byref(system)):
            return {
                "admitted": False,
                "platform": "windows",
                "reason": "GetProcessAffinityMask failed",
                "winerror": ctypes.get_last_error(),
            }
        available = original.value.bit_count()
        return {
            "admitted": available >= 2,
            "platform": "windows",
            "available_logical_cpus": available,
            "reason": (
                None
                if available >= 2
                else "isolated sampling requires at least two available logical CPUs"
            ),
        }
    if hasattr(os, "sched_getaffinity") and hasattr(os, "sched_setaffinity"):
        available = sorted(os.sched_getaffinity(0))
        return {
            "admitted": len(available) >= 2,
            "platform": sys.platform,
            "available_logical_cpus": len(available),
            "reason": (
                None
                if len(available) >= 2
                else "isolated sampling requires at least two available logical CPUs"
            ),
        }
    return {
        "admitted": False,
        "platform": sys.platform,
        "available_logical_cpus": None,
        "reason": "strict direct-child process affinity is unsupported on this platform",
    }


@contextmanager
def isolated_sampling_cpu():
    """Keep the orchestrator/guard off the logical CPU owned by timed children."""
    if sys.platform == "win32":
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        get_process = kernel32.GetCurrentProcess
        get_process.restype = ctypes.c_void_p
        get_affinity = kernel32.GetProcessAffinityMask
        get_affinity.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_size_t),
            ctypes.POINTER(ctypes.c_size_t),
        ]
        set_affinity = kernel32.SetProcessAffinityMask
        set_affinity.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
        get_priority = kernel32.GetPriorityClass
        get_priority.argtypes = [ctypes.c_void_p]
        get_priority.restype = ctypes.c_uint32
        process = get_process()
        original = ctypes.c_size_t()
        system = ctypes.c_size_t()
        if not get_affinity(process, ctypes.byref(original), ctypes.byref(system)):
            raise OSError(ctypes.get_last_error(), "GetProcessAffinityMask failed")
        available = [
            index
            for index in range(ctypes.sizeof(ctypes.c_size_t) * 8)
            if original.value & (1 << index)
        ]
        if len(available) < 2:
            raise RuntimeError(
                "isolated fresh-process sampling requires at least two available logical CPUs"
            )
        # Avoid CPU 0's conventional interrupt/control-plane load when the
        # process has a wider affinity set. The receipt names this honestly as
        # a logical CPU; topology-specific physical-core claims require OS
        # topology APIs and are deliberately not inferred here.
        selected = available[2] if len(available) > 2 else available[-1]
        selected_mask = 1 << selected
        orchestrator_mask = original.value & ~selected_mask
        if not set_affinity(process, orchestrator_mask):
            raise OSError(ctypes.get_last_error(), "SetProcessAffinityMask failed")
        original_priority = int(get_priority(process))
        above_normal_priority_class = 0x00008000
        contract = {
            "platform": "windows",
            "scope": "timed direct child only; orchestrator and guard isolated on complement",
            "child_logical_cpu": selected,
            "child_affinity_mask": selected_mask,
            "orchestrator_affinity_mask": orchestrator_mask,
            "orchestrator_original_affinity_mask": original.value,
            "system_affinity_mask": system.value,
            "orchestrator_priority_class": original_priority,
            "child_priority_class": above_normal_priority_class,
        }
        child_env = {
            "MOLT_CEXT_BENCH_TARGET_CPU": str(selected),
            "MOLT_CEXT_BENCH_TARGET_MASK": str(selected_mask),
            "MOLT_CEXT_BENCH_EXPECTED_PRIORITY": str(above_normal_priority_class),
        }
        try:
            yield contract, child_env
        finally:
            if not set_affinity(process, original.value):
                raise OSError(
                    ctypes.get_last_error(), "failed to restore benchmark process affinity"
                )
        return
    if hasattr(os, "sched_getaffinity") and hasattr(os, "sched_setaffinity"):
        available = sorted(os.sched_getaffinity(0))
        if len(available) < 2:
            raise RuntimeError(
                "isolated fresh-process sampling requires at least two available logical CPUs"
            )
        selected = available[-1]
        orchestrator_cpus = set(available) - {selected}
        os.sched_setaffinity(0, orchestrator_cpus)
        expected_priority = os.getpriority(os.PRIO_PROCESS, 0)
        contract = {
            "platform": sys.platform,
            "scope": "timed direct child only; orchestrator and guard isolated on complement",
            "child_logical_cpu": selected,
            "orchestrator_logical_cpus": sorted(orchestrator_cpus),
            "orchestrator_original_logical_cpus": available,
            "child_nice": expected_priority,
        }
        child_env = {
            "MOLT_CEXT_BENCH_TARGET_CPU": str(selected),
            "MOLT_CEXT_BENCH_EXPECTED_PRIORITY": str(expected_priority),
        }
        try:
            yield contract, child_env
        finally:
            os.sched_setaffinity(0, set(available))
        return
    raise RuntimeError(f"isolated benchmark process affinity is unsupported on {sys.platform}")


def parse_sample(text: str) -> dict[str, object]:
    matches = [
        line.split(SAMPLE_PREFIX, 1)[1]
        for line in text.splitlines()
        if SAMPLE_PREFIX in line
    ]
    if len(matches) != 1:
        raise ValueError(f"expected exactly one {SAMPLE_PREFIX.strip()} record, got {len(matches)}")
    payload = json.loads(matches[0])
    if payload.get("candidate") not in {"admitted", "checked-nested"}:
        raise ValueError(f"invalid benchmark candidate: {payload.get('candidate')!r}")
    for key in ("baseline_ns_per_call", "candidate_ns_per_call"):
        value = payload.get(key)
        if not isinstance(value, (int, float)) or not math.isfinite(value) or value <= 0:
            raise ValueError(f"invalid benchmark field {key}: {value!r}")
    pair_rounds = payload.get("pair_rounds")
    baseline_rounds = payload.get("baseline_rounds_ns_per_call")
    candidate_rounds = payload.get("candidate_rounds_ns_per_call")
    if (
        not isinstance(pair_rounds, int)
        or pair_rounds < 2
        or not isinstance(baseline_rounds, list)
        or not isinstance(candidate_rounds, list)
        or len(baseline_rounds) != pair_rounds
        or len(candidate_rounds) != pair_rounds
        or not all(
            isinstance(value, (int, float)) and math.isfinite(value) and value > 0
            for value in (*baseline_rounds, *candidate_rounds)
        )
    ):
        raise ValueError("invalid raw paired-round benchmark evidence")
    return payload


def validate_sample_process_contract(
    sample: dict[str, object],
    isolation_contract: dict[str, object],
    evidence: dict[str, object],
) -> None:
    child = sample.get("process_execution_contract")
    if not isinstance(child, dict) or child.get("verified_before_warmup") is not True:
        raise RuntimeError("timed child did not verify its process contract before warmup")
    if child.get("pid") != evidence.get("pid"):
        raise RuntimeError(
            f"timed child PID {child.get('pid')!r} did not match guard custody "
            f"PID {evidence.get('pid')!r}"
        )
    if child.get("logical_cpu") != isolation_contract.get("child_logical_cpu"):
        raise RuntimeError("timed child did not acquire the isolated logical CPU")
    if sys.platform == "win32":
        if (
            child.get("affinity_mask") != isolation_contract.get("child_affinity_mask")
            or child.get("priority_class")
            != isolation_contract.get("child_priority_class")
        ):
            raise RuntimeError("timed Windows child affinity/priority contract drifted")
    elif child.get("nice") != isolation_contract.get("child_nice"):
        raise RuntimeError("timed POSIX child priority contract drifted")


def one_sided_ratio_ucb(samples: list[dict[str, object]]) -> dict[str, float]:
    if len(samples) < 5:
        raise ValueError("at least five independent process samples are required")
    log_ratios = [
        math.log(float(sample["candidate_ns_per_call"]) / float(sample["baseline_ns_per_call"]))
        for sample in samples
    ]
    mean = statistics.fmean(log_ratios)
    standard_deviation = statistics.stdev(log_ratios)
    degrees_of_freedom = len(log_ratios) - 1
    critical = _T95[degrees_of_freedom] if degrees_of_freedom < len(_T95) else 1.645
    upper_log_ratio = mean + critical * standard_deviation / math.sqrt(len(log_ratios))
    return {
        "geometric_mean_delta_pct": (math.exp(mean) - 1.0) * 100.0,
        "one_sided_95_ucb_delta_pct": (math.exp(upper_log_ratio) - 1.0) * 100.0,
        "log_ratio_standard_deviation": standard_deviation,
        "student_t_critical": critical,
    }


def _write_json_atomic(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    temporary.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(temporary, path)


def _run_with_evidence(
    command: list[str],
    *,
    run_dir: Path,
    stem: str,
    timeout: float,
    env: dict[str, str] | None = None,
):
    stdout_path = run_dir / f"{stem}.stdout"
    stderr_path = run_dir / f"{stem}.stderr"
    result = run_completed_command(
        command,
        cwd=ROOT,
        env=env,
        capture_output=True,
        stdout_capture_path=stdout_path,
        stderr_capture_path=stderr_path,
        capture_tail_bytes=64 * 1024,
        memory_guard_prefix="MOLT_CEXT_BENCH",
        timeout=timeout,
        text=True,
    )
    evidence = {
        "stdout_path": str(stdout_path),
        "stdout_bytes": stdout_path.stat().st_size,
        "stdout_sha256": _sha256(stdout_path),
        "stderr_path": str(stderr_path),
        "stderr_bytes": stderr_path.stat().st_size,
        "stderr_sha256": _sha256(stderr_path),
        "elapsed_s": getattr(result, "elapsed_s", None),
        "returncode": result.returncode,
    }
    child = getattr(result, "child_process", None)
    if child is not None:
        evidence["pid"] = getattr(child, "pid", None)
    peak = getattr(result, "peak", None)
    if peak is not None:
        evidence["peak_process_rss_kb"] = getattr(peak, "rss_kb", None)
    peak_total = getattr(result, "peak_total", None)
    if peak_total is not None:
        evidence["peak_tree_rss_kb"] = getattr(peak_total, "rss_kb", None)
    if result.returncode != 0:
        raise RuntimeError(
            f"guarded command failed ({result.returncode}): {command!r}; "
            f"stderr evidence: {stderr_path}"
        )
    return result, evidence


def _discover_release_test_binary(run_dir: Path, timeout: float) -> tuple[Path, dict[str, object]]:
    command = [
        "cargo",
        "test",
        "--manifest-path",
        str(ROOT / "runtime" / "Cargo.toml"),
        "-p",
        "molt-runtime",
        "--lib",
        "--release",
        "--features",
        "l7-attestation-probe",
        "--no-run",
        "--message-format=json-render-diagnostics",
    ]
    _result, evidence = _run_with_evidence(
        command, run_dir=run_dir, stem="build", timeout=timeout
    )
    stdout_path = Path(str(evidence["stdout_path"]))
    executables: list[Path] = []
    with stdout_path.open("r", encoding="utf-8") as stream:
        for line in stream:
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if event.get("reason") != "compiler-artifact" or not event.get("executable"):
                continue
            target = event.get("target") or {}
            profile = event.get("profile") or {}
            if target.get("name") in {"molt-runtime", "molt_runtime"} and profile.get("test"):
                executables.append(Path(event["executable"]))
    unique = sorted({path.resolve() for path in executables})
    if len(unique) != 1:
        raise RuntimeError(f"expected exactly one molt-runtime release test binary, got {unique!r}")
    return unique[0], {"command": command, "evidence": evidence}


def benchmark(
    *,
    samples: int,
    iterations: int,
    threshold_pct: float,
    build_timeout: float,
    sample_timeout: float,
    output: Path,
) -> dict[str, object]:
    if samples < 5:
        raise ValueError("--samples must be at least 5")
    if iterations < 100_000:
        raise ValueError("--iterations must be at least 100000")
    if iterations % 120 != 0:
        raise ValueError("--iterations must be divisible by the 120 paired rounds")
    platform_admission = sampling_platform_admission()
    if platform_admission["admitted"] is not True:
        payload: dict[str, object] = {
            "schema_version": 2,
            "admitted": False,
            "passed": False,
            "status": "not-admitted",
            "platform_admission": platform_admission,
            "required_capability": (
                "two disjoint logical CPUs plus strict direct-child process affinity"
            ),
        }
        _write_json_atomic(output, payload)
        return payload
    run_id = f"{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}-{uuid.uuid4().hex[:12]}"
    run_dir = output.parent / "runs" / run_id
    run_dir.mkdir(parents=True, exist_ok=False)
    binary, build = _discover_release_test_binary(run_dir, build_timeout)
    identity = executable_identity(binary)
    raw_samples: dict[str, list[dict[str, object]]] = {
        "admitted": [],
        "checked-nested": [],
    }
    with isolated_sampling_cpu() as (sampling_isolation_contract, child_env):
        for index in range(samples):
            for candidate in raw_samples:
                if executable_identity(binary) != identity:
                    raise RuntimeError(
                        "benchmark executable changed after build and before direct launch"
                    )
                order = "baseline-first" if index % 2 == 0 else "candidate-first"
                env = dict(os.environ)
                env.update(child_env)
                env["MOLT_CEXT_BENCH_CANDIDATE"] = candidate
                env["MOLT_CEXT_BENCH_ORDER"] = order
                env["MOLT_CEXT_BENCH_ITERATIONS"] = str(iterations)
                command = [
                    str(binary),
                    TEST_NAME,
                    "--ignored",
                    "--exact",
                    "--nocapture",
                    "--test-threads=1",
                ]
                _result, evidence = _run_with_evidence(
                    command,
                    run_dir=run_dir,
                    stem=f"sample-{candidate}-{index:02d}",
                    timeout=sample_timeout,
                    env=env,
                )
                sample = parse_sample(
                    Path(str(evidence["stdout_path"])).read_text(encoding="utf-8")
                )
                if (
                    sample.get("candidate") != candidate
                    or sample.get("order") != order
                    or sample.get("iterations") != iterations
                ):
                    raise RuntimeError(
                        f"sample {candidate}/{index} did not honor its process contract: {sample!r}"
                    )
                if sample.get("allocation_probe_enabled") is not True:
                    raise RuntimeError("release sample was not built with the allocation probe")
                validate_sample_process_contract(
                    sample, sampling_isolation_contract, evidence
                )
                sample["sample_index"] = index
                sample["executable_identity"] = identity
                sample["process_evidence"] = evidence
                raw_samples[candidate].append(sample)
    if executable_identity(binary) != identity:
        raise RuntimeError("benchmark executable changed during direct fresh-process sampling")

    statistics_payload = {
        candidate: one_sided_ratio_ucb(candidate_samples)
        for candidate, candidate_samples in raw_samples.items()
    }
    allocation_deltas = {
        candidate: [
            int(sample["candidate_allocations"])
            - int(sample["baseline_allocations"])
            for sample in candidate_samples
        ]
        for candidate, candidate_samples in raw_samples.items()
    }
    passed = all(
        statistics_payload[candidate]["one_sided_95_ucb_delta_pct"] < threshold_pct
        and max(allocation_deltas[candidate]) <= 0
        for candidate in raw_samples
    )
    payload: dict[str, object] = {
        "schema_version": 2,
        "admitted": True,
        "platform_admission": platform_admission,
        "run_id": run_id,
        "method": {
            "independent_unit": "one fresh direct test-binary process per paired sample",
            "pair_order": "balanced ABBA+BAAB quartets, with process starting orientation alternated",
            "statistic": "one-sided 95% Student-t UCB over paired log ratios",
            "threshold_pct": threshold_pct,
            "candidates": ["admitted", "checked-nested"],
            "allocation_rule": "candidate allocation count must not exceed baseline in any process",
        },
        "build": build,
        "sampling_isolation_contract": sampling_isolation_contract,
        "executable_identity": identity,
        "samples": raw_samples,
        "statistics": statistics_payload,
        "allocation_delta_counts": allocation_deltas,
        "passed": passed,
    }
    _write_json_atomic(output, payload)
    if not passed:
        ucbs = {
            candidate: stats["one_sided_95_ucb_delta_pct"]
            for candidate, stats in statistics_payload.items()
        }
        raise RuntimeError(
            f"C-extension trampoline benchmark failed: UCBs={ucbs} "
            f"threshold={threshold_pct:.6f}%, allocation_deltas={allocation_deltas}; "
            f"receipt={output}"
        )
    return payload


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=int, default=31)
    parser.add_argument("--iterations", type=int, default=1_200_000)
    parser.add_argument("--threshold-pct", type=float, default=1.0)
    parser.add_argument("--build-timeout", type=float, default=2400.0)
    parser.add_argument("--sample-timeout", type=float, default=120.0)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    payload = benchmark(
        samples=args.samples,
        iterations=args.iterations,
        threshold_pct=args.threshold_pct,
        build_timeout=args.build_timeout,
        sample_timeout=args.sample_timeout,
        output=args.out.resolve(),
    )
    if payload.get("admitted") is not True:
        admission = payload["platform_admission"]
        print(
            "C-extension trampoline benchmark not admitted: "
            f"platform={admission['platform']} reason={admission['reason']} "
            f"receipt={args.out.resolve()}",
            file=sys.stderr,
        )
        return 2
    stats = payload["statistics"]
    ucb_summary = ", ".join(
        f"{candidate}={candidate_stats['one_sided_95_ucb_delta_pct']:.6f}%"
        for candidate, candidate_stats in stats.items()
    )
    print(
        f"C-extension trampoline UCBs: {ucb_summary} "
        f"receipt={args.out.resolve()}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
