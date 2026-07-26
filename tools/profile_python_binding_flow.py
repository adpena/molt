#!/usr/bin/env python3
"""Profile the canonical Python binding/capability analysis hot path."""

from __future__ import annotations

import argparse
import ast
import ctypes
import gc
import hashlib
import json
import math
import os
import platform
import statistics
import subprocess
import sys
import time
import tracemalloc
from pathlib import Path
from typing import cast

ROOT = Path(__file__).resolve().parents[1]
IMPLEMENTATION_ROOT = Path(
    os.environ.get("MOLT_BINDING_PROFILE_SOURCE_ROOT", ROOT)
).resolve()
IMPLEMENTATION_SRC = IMPLEMENTATION_ROOT / "src"
sys.path.insert(0, str(IMPLEMENTATION_SRC))

from molt.compiler_analysis.python_binding_flow import (  # noqa: E402
    PythonBindingPolicy,
    analyze_python_bindings,
    analyze_python_source_bindings,
    python_source_digest,
)

try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor

_COMMANDS = CommandExecutor.for_file(__file__)


def _representative_source(
    import_count: int,
    deferred_count: int,
    type_alias_count: int,
    nested_sibling_count: int,
) -> str:
    lines = ["import importlib as loader", "from importlib import import_module"]
    for index in range(import_count):
        lines.extend(
            (
                f"target_{index} = loader.import_module",
                f"if flag_{index}:",
                f"    target_{index} = replacement_{index}",
                f"target_{index}('package.module_{index}')",
            )
        )
    for index in range(deferred_count):
        lines.extend(
            (
                f"def outer_{index}():",
                f"    def inner_{index}():",
                f"        deferred_{index}('package.deferred_{index}')",
                f"    from importlib import import_module as deferred_{index}",
            )
        )
    for index in range(type_alias_count):
        lines.extend(
            (
                f"type Alias_{index} = lazy_{index}('package.lazy_{index}')",
                f"from importlib import import_module as lazy_{index}",
            )
        )
    if nested_sibling_count:
        lines.append("def shared_outer():")
        for index in range(nested_sibling_count):
            lines.extend(
                (
                    f"    def sibling_{index}():",
                    f"        sibling_loader_{index}('package.sibling_{index}')",
                )
            )
        for index in range(nested_sibling_count):
            lines.append(
                f"    from importlib import import_module as sibling_loader_{index}"
            )
    return "\n".join(lines) + "\n"


def _percentile(samples: list[int], percentile: float) -> int:
    ordered = sorted(samples)
    index = min(len(ordered) - 1, round((len(ordered) - 1) * percentile))
    return ordered[index]


def _process_memory_bytes() -> tuple[int | None, int | None]:
    if os.name == "nt":

        class ProcessMemoryCounters(ctypes.Structure):
            _fields_ = [
                ("cb", ctypes.c_ulong),
                ("PageFaultCount", ctypes.c_ulong),
                ("PeakWorkingSetSize", ctypes.c_size_t),
                ("WorkingSetSize", ctypes.c_size_t),
                ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                ("PagefileUsage", ctypes.c_size_t),
                ("PeakPagefileUsage", ctypes.c_size_t),
            ]

        counters = ProcessMemoryCounters()
        counters.cb = ctypes.sizeof(counters)
        kernel32 = ctypes.windll.kernel32
        kernel32.GetCurrentProcess.restype = ctypes.c_void_p
        kernel32.K32GetProcessMemoryInfo.argtypes = (
            ctypes.c_void_p,
            ctypes.POINTER(ProcessMemoryCounters),
            ctypes.c_ulong,
        )
        kernel32.K32GetProcessMemoryInfo.restype = ctypes.c_int
        if not kernel32.K32GetProcessMemoryInfo(
            kernel32.GetCurrentProcess(),
            ctypes.byref(counters),
            counters.cb,
        ):
            return None, None
        return int(counters.WorkingSetSize), int(counters.PeakWorkingSetSize)

    try:
        import resource
    except ImportError:
        return None, None
    peak = int(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)
    if peak <= 0:
        return None, None
    peak_bytes = peak if sys.platform == "darwin" else peak * 1024
    return None, peak_bytes


def _file_digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _identity(*, label: str) -> dict[str, object]:
    implementation_path = Path(
        sys.modules[analyze_python_bindings.__module__].__file__ or ""
    )
    try:
        git_head = _COMMANDS.run(
            ["git", "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
            cwd=IMPLEMENTATION_ROOT,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        git_head = None
    implementation_path = implementation_path.resolve()
    if not implementation_path.is_relative_to(IMPLEMENTATION_SRC):
        raise RuntimeError(
            "binding profile imported analysis outside selected source root: "
            f"{implementation_path} is not under {IMPLEMENTATION_SRC}"
        )
    return {
        "label": label,
        "git_head": git_head,
        "python_version": platform.python_version(),
        "python_implementation": platform.python_implementation(),
        "python_executable": sys.executable,
        "os": platform.platform(),
        "machine": platform.machine(),
        "processor": platform.processor() or os.environ.get("PROCESSOR_IDENTIFIER"),
        "process_id": os.getpid(),
        "tool_path": str(Path(__file__).resolve()),
        "tool_sha256": _file_digest(Path(__file__).resolve()),
        "selected_source_root": str(IMPLEMENTATION_ROOT),
        "implementation_path": str(implementation_path),
        "implementation_sha256": _file_digest(implementation_path),
    }


def _analysis_telemetry(index: object) -> dict[str, int] | None:
    telemetry = getattr(index, "telemetry", None)
    if telemetry is None:
        return None
    return {
        "binding_lookups": telemetry.binding_lookups,
        "join_calls": telemetry.join_calls,
        "join_node_visits": telemetry.join_node_visits,
        "join_shared_subtrees_skipped": telemetry.join_shared_subtrees_skipped,
        "join_chunk_merges": telemetry.join_chunk_merges,
        "structural_diff_cache_entries": telemetry.structural_diff_cache_entries,
        "structural_diff_node_visits": telemetry.structural_diff_node_visits,
        "structural_diff_shared_subtrees_skipped": (
            telemetry.structural_diff_shared_subtrees_skipped
        ),
    }


def _measure_once(
    import_count: int,
    deferred_count: int,
    type_alias_count: int,
    nested_sibling_count: int,
    iterations: int,
    *,
    implementation_label: str,
) -> dict[str, object]:
    baseline_current_rss, baseline_peak_rss = _process_memory_bytes()
    source = _representative_source(
        import_count,
        deferred_count,
        type_alias_count,
        nested_sibling_count,
    )
    digest = python_source_digest(source)
    tree = ast.parse(source, feature_version=(3, 12))
    policy = PythonBindingPolicy()

    # Warm Python's own allocators and validate the representative shape first.
    reference = analyze_python_bindings(
        tree, source_digest=f"{digest}:reference", policy=policy
    )
    expected_calls = (
        import_count + deferred_count + type_alias_count + nested_sibling_count
    )
    assert len(reference.calls) == expected_calls

    analysis_ns: list[int] = []
    for iteration in range(iterations):
        start = time.perf_counter_ns()
        index = analyze_python_bindings(
            tree,
            source_digest=f"{digest}:analysis:{iteration}",
            policy=policy,
        )
        analysis_ns.append(time.perf_counter_ns() - start)
        assert len(index.calls) == expected_calls

    parse_analysis_ns: list[int] = []
    for iteration in range(iterations):
        start = time.perf_counter_ns()
        parsed = ast.parse(source, feature_version=(3, 12))
        index = analyze_python_bindings(
            parsed,
            source_digest=f"{digest}:parse-analysis:{iteration}",
            policy=policy,
        )
        parse_analysis_ns.append(time.perf_counter_ns() - start)
        assert len(index.calls) == expected_calls

    gc.collect()
    tracemalloc.start(8)
    try:
        allocation_start = time.perf_counter_ns()
        allocation_index = analyze_python_bindings(
            tree,
            source_digest=f"{digest}:allocation",
            policy=policy,
        )
        allocation_profile_ns = time.perf_counter_ns() - allocation_start
        assert len(allocation_index.calls) == expected_calls
        gc.collect()
        retained_bytes, peak_bytes = tracemalloc.get_traced_memory()
    finally:
        tracemalloc.stop()

    cached = analyze_python_source_bindings(source, policy=policy)
    cached_ns: list[int] = []
    hash_ns: list[int] = []
    for _iteration in range(iterations * 10):
        start = time.perf_counter_ns()
        assert python_source_digest(source) == digest
        hash_ns.append(time.perf_counter_ns() - start)
        start = time.perf_counter_ns()
        hit = analyze_python_source_bindings(source, policy=policy)
        cached_ns.append(time.perf_counter_ns() - start)
        assert hit is cached

    final_current_rss, final_peak_rss = _process_memory_bytes()
    median = int(statistics.median(analysis_ns))
    parse_analysis_median = int(statistics.median(parse_analysis_ns))
    node_count = sum(1 for _node in ast.walk(tree))
    return {
        "identity": _identity(label=implementation_label),
        "source_sha256": hashlib.sha256(source.encode("utf-8")).hexdigest(),
        "source_bytes": len(source.encode("utf-8")),
        "ast_nodes": node_count,
        "import_calls": import_count,
        "deferred_calls": deferred_count,
        "type_alias_calls": type_alias_count,
        "nested_sibling_calls": nested_sibling_count,
        "states": reference.state_count,
        "analysis_telemetry": _analysis_telemetry(reference),
        "scopes": len(reference.scopes),
        "iterations": iterations,
        "analysis_only": {
            "median_ns": median,
            "p95_ns": _percentile(analysis_ns, 0.95),
            "nodes_per_second": round(node_count * 1_000_000_000 / median),
            "ns_per_ast_node": round(median / node_count, 2),
        },
        "parse_and_analysis": {
            "median_ns": parse_analysis_median,
            "p95_ns": _percentile(parse_analysis_ns, 0.95),
            "nodes_per_second": round(
                node_count * 1_000_000_000 / parse_analysis_median
            ),
            "ns_per_ast_node": round(parse_analysis_median / node_count, 2),
        },
        "allocation_trace": {
            "analysis_ns": allocation_profile_ns,
            "peak_traced_bytes": peak_bytes,
            "peak_bytes_per_ast_node": round(peak_bytes / node_count, 2),
            "retained_index_bytes": retained_bytes,
            "retained_bytes_per_ast_node": round(retained_bytes / node_count, 2),
        },
        "isolated_process_memory": {
            "baseline_current_rss_bytes": baseline_current_rss,
            "baseline_peak_rss_bytes": baseline_peak_rss,
            "final_current_rss_bytes": final_current_rss,
            "final_peak_rss_bytes": final_peak_rss,
            "peak_rss_delta_from_start_bytes": (
                None
                if baseline_peak_rss is None or final_peak_rss is None
                else max(0, final_peak_rss - baseline_peak_rss)
            ),
        },
        "cache_hit": {
            "median_ns": int(statistics.median(cached_ns)),
            "p95_ns": _percentile(cached_ns, 0.95),
            "same_immutable_index": True,
            "includes_source_hash": True,
        },
        "source_hash": {
            "median_ns": int(statistics.median(hash_ns)),
            "p95_ns": _percentile(hash_ns, 0.95),
        },
    }


def _fit_log_log_slope(
    samples: list[dict[str, object]], metric: str, field: str
) -> float:
    xs = [math.log(float(cast(int, sample["ast_nodes"]))) for sample in samples]
    ys = [
        math.log(float(cast(int, cast(dict[str, object], sample[metric])[field])))
        for sample in samples
    ]
    x_mean = statistics.fmean(xs)
    y_mean = statistics.fmean(ys)
    numerator = sum((x - x_mean) * (y - y_mean) for x, y in zip(xs, ys))
    denominator = sum((x - x_mean) ** 2 for x in xs)
    return numerator / denominator if denominator else 0.0


def _fit_nonnegative_log_log_slope(
    samples: list[dict[str, object]], metric: str, field: str
) -> float | None:
    raw_values = [cast(dict[str, object], sample[metric])[field] for sample in samples]
    if any(value is None for value in raw_values):
        return None
    values = [int(cast(int, value)) for value in raw_values]
    if all(value == 0 for value in values):
        return 0.0
    if any(value <= 0 for value in values):
        return None
    return _fit_log_log_slope(samples, metric, field)


def _run_isolated_sample(
    *,
    args: argparse.Namespace,
    scale: int,
) -> dict[str, object]:
    command = [
        sys.executable,
        str(Path(__file__).resolve()),
        "--internal-sample",
        "--imports",
        str(args.imports * scale),
        "--deferred",
        str(args.deferred * scale),
        "--type-aliases",
        str(args.type_aliases * scale),
        "--nested-siblings",
        str(args.nested_siblings * scale),
        "--iterations",
        str(args.iterations),
        "--implementation-label",
        args.implementation_label,
    ]
    child_env = os.environ.copy()
    child_env["MOLT_BINDING_PROFILE_SOURCE_ROOT"] = str(IMPLEMENTATION_ROOT)
    child_env["MOLT_PROJECT_ROOT"] = str(IMPLEMENTATION_ROOT)
    child_env["PYTHONPATH"] = str(IMPLEMENTATION_SRC)
    completed = _COMMANDS.run(
        command,
        check=False,
        capture_output=True,
        text=True,
        env=child_env,
        cwd=IMPLEMENTATION_ROOT,
    )
    if completed.returncode:

        def bounded(value: str, limit: int = 8_000) -> str:
            return value if len(value) <= limit else value[-limit:]

        raise RuntimeError(
            json.dumps(
                {
                    "command": command,
                    "returncode": completed.returncode,
                    "stdout": bounded(completed.stdout),
                    "stderr": bounded(completed.stderr),
                },
                sort_keys=True,
            )
        )
    return cast(dict[str, object], json.loads(completed.stdout))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imports", type=int, default=10)
    parser.add_argument("--deferred", type=int, default=10)
    parser.add_argument("--type-aliases", type=int, default=10)
    parser.add_argument("--nested-siblings", type=int, default=10)
    parser.add_argument("--iterations", type=int, default=3)
    parser.add_argument("--scales", default="1,2,4,8")
    parser.add_argument("--max-analysis-slope", type=float, default=1.35)
    parser.add_argument("--implementation-label", default="candidate")
    parser.add_argument("--workload-name", default="mixed-import-binding-authority-v1")
    parser.add_argument("--json", type=Path)
    parser.add_argument(
        "--internal-sample", action="store_true", help=argparse.SUPPRESS
    )
    args = parser.parse_args()
    if (
        args.imports <= 0
        or args.deferred < 0
        or args.type_aliases < 0
        or args.nested_siblings < 0
        or args.iterations <= 0
    ):
        parser.error(
            "--imports and --iterations must be positive; deferred counts "
            "must be non-negative"
        )
    if args.internal_sample:
        payload = _measure_once(
            args.imports,
            args.deferred,
            args.type_aliases,
            args.nested_siblings,
            args.iterations,
            implementation_label=args.implementation_label,
        )
        print(json.dumps(payload, sort_keys=True))
        return 0
    try:
        scales = [int(value) for value in args.scales.split(",")]
    except ValueError:
        parser.error("--scales must be a comma-separated list of positive integers")
    if (
        not scales
        or any(scale <= 0 for scale in scales)
        or len(set(scales)) != len(scales)
    ):
        parser.error("--scales must contain unique positive integers")
    if len(scales) < 3:
        parser.error("performance claims require at least three geometric scales")
    samples = [_run_isolated_sample(args=args, scale=scale) for scale in scales]
    scaling: dict[str, object] = {
        "scales": scales,
        "max_analysis_slope": args.max_analysis_slope,
        "analysis_slope": None,
        "parse_and_analysis_slope": None,
        "peak_traced_allocation_slope": None,
        "retained_index_slope": None,
        "peak_rss_delta_slope": None,
        "passes": True,
    }
    if len(samples) >= 3:
        analysis_slope = _fit_log_log_slope(samples, "analysis_only", "median_ns")
        parse_analysis_slope = _fit_log_log_slope(
            samples, "parse_and_analysis", "median_ns"
        )
        allocation_slope = _fit_log_log_slope(
            samples, "allocation_trace", "peak_traced_bytes"
        )
        retained_slope = _fit_nonnegative_log_log_slope(
            samples, "allocation_trace", "retained_index_bytes"
        )
        rss_slope = _fit_nonnegative_log_log_slope(
            samples,
            "isolated_process_memory",
            "peak_rss_delta_from_start_bytes",
        )
        scaling.update(
            analysis_slope=round(analysis_slope, 4),
            parse_and_analysis_slope=round(parse_analysis_slope, 4),
            peak_traced_allocation_slope=round(allocation_slope, 4),
            retained_index_slope=(
                None if retained_slope is None else round(retained_slope, 4)
            ),
            peak_rss_delta_slope=(None if rss_slope is None else round(rss_slope, 4)),
            passes=(
                analysis_slope <= args.max_analysis_slope
                and parse_analysis_slope <= args.max_analysis_slope
                and allocation_slope <= args.max_analysis_slope
                and retained_slope is not None
                and retained_slope <= args.max_analysis_slope
                and rss_slope is not None
                and rss_slope <= args.max_analysis_slope
            ),
        )
    payload = {
        "schema_version": 2,
        "workload_family": {
            "name": args.workload_name,
            "imports": args.imports,
            "deferred": args.deferred,
            "type_aliases": args.type_aliases,
            "nested_siblings": args.nested_siblings,
            "iterations_per_scale": args.iterations,
        },
        "scaling": scaling,
        "samples": samples,
    }
    encoded = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    if args.json is not None:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0 if scaling["passes"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
