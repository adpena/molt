#!/usr/bin/env python3
"""Reproducible native-link plan, execution, and finalization benchmark.

This is the single linker measurement authority.  It consumes the production
``NativeLinkPlan`` and production candidate finalizer, executes the exact plan
through the shared process/memory guard, and refuses baseline comparisons when
the host, plan, inputs, or resolved toolchain have drifted.

The measured hot path is linear in total input bytes plus linker graph work:
O(input bytes + symbols + sections + relocations).  Plan construction is
O(command arguments + declared inputs); publication is O(output bytes).
"""

from __future__ import annotations

import argparse
from collections import Counter
from dataclasses import asdict
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import sys
import time
import tracemalloc
from typing import Callable, Mapping, Sequence
import uuid


ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
for import_root in (ROOT, SRC):
    if str(import_root) not in sys.path:
        sys.path.insert(0, str(import_root))

from molt.cli.build_results import _finalize_native_link_candidate  # noqa: E402
from molt.cli.link_pipeline import _native_link_execution_command  # noqa: E402
from molt.cli.native_link_command import _build_native_link_plan  # noqa: E402
from molt.cli.native_link_plan import NativeLinkPlan  # noqa: E402
from molt.cli.native_link_manifest import (  # noqa: E402
    native_link_dependency_manifest_path,
    read_native_link_dependency_manifest,
)
from molt.cli.native_link_tool_identity import native_link_tool_facts  # noqa: E402
from tools import harness_memory_guard, perf_calibration  # noqa: E402
try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)


SCHEMA_VERSION = 1
KIND = "molt_native_link_benchmark"
SUPPORTED_OSES = frozenset({"linux", "macos", "windows"})
SUPPORTED_ARCHES = frozenset({"x86_64", "aarch64"})
IDENTITY_FIELDS = (
    "host_fingerprint",
    "plan_fingerprint",
    "input_fingerprint",
    "tool_fingerprint",
    "measurement_fingerprint",
    "comparison_fingerprint",
)


class LinkBenchmarkError(RuntimeError):
    """A benchmark contract or measured phase failed."""

    def __init__(
        self, message: str, *, report: Mapping[str, object] | None = None
    ) -> None:
        super().__init__(message)
        self.report = dict(report) if report is not None else None


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def _stable_hash(payload: object) -> str:
    encoded = json.dumps(
        {"fingerprint_schema_version": 1, "payload": payload},
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=True,
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def measurement_authority_fingerprint() -> str:
    """Content-identify the code that defines measurement semantics."""
    sources = (
        ("benchmark", Path(__file__)),
        ("process_memory_guard", ROOT / "tools" / "harness_memory_guard.py"),
        ("quiescence", ROOT / "tools" / "perf_calibration.py"),
        (
            "tool_identity",
            SRC / "molt" / "cli" / "native_link_tool_identity.py",
        ),
    )
    return _stable_hash(
        [{"role": role, "sha256": _sha256_file(path)} for role, path in sources]
    )


def implementation_source_facts() -> dict[str, object]:
    """Record the primary native-link authorities as the experimental treatment."""
    sources = (
        "native_link_plan.py",
        "native_link_command.py",
        "native_link_deps.py",
        "native_link_manifest.py",
        "link_pipeline.py",
        "build_results.py",
        "runtime_build.py",
    )
    files = [
        {
            "name": name,
            "sha256": _sha256_file(SRC / "molt" / "cli" / name),
        }
        for name in sources
    ]
    return {"files": files, "fingerprint": _stable_hash(files)}


def _canonical_arch(raw: str) -> str:
    normalized = raw.strip().lower().replace(" ", "_")
    return {
        "amd64": "x86_64",
        "x64": "x86_64",
        "arm64": "aarch64",
    }.get(normalized, normalized)


def host_payload() -> dict[str, object]:
    payload: dict[str, object] = {
        "os": {"win32": "windows", "darwin": "macos"}.get(
            sys.platform, "linux" if sys.platform.startswith("linux") else sys.platform
        ),
        "os_release": platform.release(),
        "arch": _canonical_arch(platform.machine()),
        "machine": platform.machine(),
        "processor": platform.processor() or "unknown",
        "logical_cpus": os.cpu_count(),
        "python": platform.python_version(),
        "pointer_bits": 8 * __import__("struct").calcsize("P"),
    }
    payload["fingerprint"] = _stable_hash(payload)
    return payload


def collect_input_facts(inputs: Mapping[str, Path]) -> dict[str, object]:
    files: list[dict[str, object]] = []
    for role, path in sorted(inputs.items()):
        resolved = path.expanduser().resolve(strict=True)
        if not resolved.is_file():
            raise LinkBenchmarkError(f"link input is not a file: {resolved}")
        before = resolved.stat()
        sha256 = _sha256_file(resolved)
        after = resolved.stat()
        if (before.st_size, before.st_mtime_ns) != (
            after.st_size,
            after.st_mtime_ns,
        ):
            raise LinkBenchmarkError(f"link input changed while hashing: {resolved}")
        files.append(
            {
                "role": role,
                "path": str(resolved),
                "size_bytes": after.st_size,
                "sha256": sha256,
            }
        )
    identity = [
        {key: fact[key] for key in ("role", "size_bytes", "sha256")} for fact in files
    ]
    return {
        "count": len(files),
        "total_bytes": sum(int(fact["size_bytes"]) for fact in files),
        "files": files,
        "fingerprint": _stable_hash(identity),
    }


def response_file_inputs(command: Sequence[str]) -> dict[str, Path]:
    """Return every existing response file named by the canonical plan."""
    response_files: dict[str, Path] = {}
    for argument in command:
        raw = argument
        if raw.startswith("-Wl,@"):
            raw = raw[len("-Wl,@") :]
        elif raw.startswith("@"):
            raw = raw[1:]
        else:
            continue
        path = Path(raw).expanduser()
        if path.is_file():
            response_files[f"response_{len(response_files)}"] = path
    return response_files


def plan_auxiliary_inputs(
    command: Sequence[str], *, excluded: Sequence[Path] = ()
) -> dict[str, Path]:
    """Find generated scripts/response files consumed by a native link plan."""
    excluded_keys = {
        os.path.normcase(str(path.resolve(strict=False))) for path in excluded
    }
    result: dict[str, Path] = {}
    for argument in command[1:]:
        candidates = [argument]
        if argument.startswith("-Wl,"):
            candidates.extend(argument[len("-Wl,") :].split(","))
        if "=" in argument:
            candidates.append(argument.rsplit("=", 1)[1])
        if argument.startswith("/DEF:"):
            candidates.append(argument[len("/DEF:") :])
        if argument.startswith("@"):
            candidates.append(argument[1:])
        for raw in candidates:
            if raw.startswith("/DEF:"):
                raw = raw[len("/DEF:") :]
            if raw.startswith("@"):
                raw = raw[1:]
            path = Path(raw).expanduser()
            if not path.is_file():
                continue
            resolved = path.resolve()
            key = os.path.normcase(str(resolved))
            if key in excluded_keys or any(
                os.path.normcase(str(existing.resolve())) == key
                for existing in result.values()
            ):
                continue
            result[f"plan_auxiliary_{len(result)}"] = resolved
    return result


def plan_library_inputs(command: Sequence[str]) -> dict[str, Path]:
    """Resolve libraries from plan-owned explicit search directories."""
    search_dirs: list[Path] = []
    library_names: list[str] = []
    index = 1
    while index < len(command):
        argument = command[index]
        if argument == "-L" and index + 1 < len(command):
            search_dirs.append(Path(command[index + 1]))
            index += 2
            continue
        if argument.startswith("-L") and len(argument) > 2:
            search_dirs.append(Path(argument[2:]))
        elif argument == "-l" and index + 1 < len(command):
            library_names.append(command[index + 1])
            index += 2
            continue
        elif argument.startswith("-l") and len(argument) > 2:
            library_names.append(argument[2:])
        index += 1

    result: dict[str, Path] = {}
    for library in library_names:
        name = library.split("=", 1)[-1]
        candidates = (
            f"{name}.lib",
            f"lib{name}.a",
            f"lib{name}.so",
            f"lib{name}.dylib",
        )
        for directory in search_dirs:
            match = next(
                (
                    (directory / candidate).resolve()
                    for candidate in candidates
                    if (directory / candidate).is_file()
                ),
                None,
            )
            if match is not None:
                result[f"plan_library_{len(result)}_{name}"] = match
                break
    return result


def _replace_path_tokens(value: str, replacements: Mapping[str, str]) -> str:
    normalized = os.path.normcase(os.path.normpath(value))
    ordered = sorted(replacements.items(), key=lambda item: len(item[0]), reverse=True)
    result = value
    for raw_path, token in ordered:
        normalized_path = os.path.normcase(os.path.normpath(raw_path))
        if normalized_path in normalized:
            # Preserve linker flag prefixes while removing machine-local roots.
            start = normalized.index(normalized_path)
            result = result[:start] + token + result[start + len(raw_path) :]
            normalized = os.path.normcase(os.path.normpath(result))
    return result


def normalized_plan_payload(
    plan: NativeLinkPlan,
    *,
    inputs: Mapping[str, Path],
    output: Path,
) -> dict[str, object]:
    replacements = {
        str(ROOT.resolve()): "{repo}",
        str(output.parent.resolve()): "{fixture}",
        str(output.resolve(strict=False)): "{output}",
        **{
            str(path.resolve(strict=False)): f"{{input:{role}}}"
            for role, path in inputs.items()
        },
    }
    command = [_replace_path_tokens(arg, replacements) for arg in plan.command]
    return {
        "target": asdict(plan.target),
        "capabilities": asdict(plan.capabilities),
        "policy": asdict(plan.policy),
        "command": command,
        "linker_hint": plan.linker_hint,
        "normalized_target": plan.normalized_target,
    }


def profile_plan(
    factory: Callable[[], NativeLinkPlan], *, samples: int = 6
) -> tuple[NativeLinkPlan, dict[str, object]]:
    if samples < 2:
        raise ValueError("plan profiling requires at least two samples")
    plans: list[NativeLinkPlan] = []
    wall_samples_ns: list[int] = []
    for _ in range(samples):
        started = time.perf_counter_ns()
        plans.append(factory())
        wall_samples_ns.append(time.perf_counter_ns() - started)
    plan = plans[0]
    if any(candidate != plan for candidate in plans[1:]):
        raise LinkBenchmarkError("native link plan changed across timing samples")
    warm_samples = wall_samples_ns[1:]
    warm_median = int(statistics.median(warm_samples))
    warm_mad = int(
        statistics.median(abs(value - warm_median) for value in warm_samples)
    )
    warm_relative_mad = warm_mad / warm_median if warm_median else None
    # One frame is sufficient for aggregate plan allocation facts and keeps the
    # probe from perturbing an external-tool baseline by an order of magnitude.
    tracemalloc.start(1)
    tracemalloc.reset_peak()
    before = tracemalloc.take_snapshot()
    allocation_started = time.perf_counter_ns()
    try:
        allocation_plan = factory()
        allocation_probe_wall_ns = time.perf_counter_ns() - allocation_started
        current_bytes, peak_bytes = tracemalloc.get_traced_memory()
        after = tracemalloc.take_snapshot()
    finally:
        tracemalloc.stop()
    if allocation_plan != plan:
        raise LinkBenchmarkError("native link plan changed during allocation probe")
    diff = after.compare_to(before, "lineno")
    return plan, {
        "wall_ns": warm_median,
        "cold_wall_ns": wall_samples_ns[0],
        "warm_wall_samples_ns": warm_samples,
        "warm_wall_ns_median": warm_median,
        "warm_wall_ns_mad": warm_mad,
        "warm_wall_relative_mad": warm_relative_mad,
        "stable": (
            len(warm_samples) >= 5
            and warm_relative_mad is not None
            and warm_relative_mad <= 0.05
        ),
        "allocation_probe_wall_ns": allocation_probe_wall_ns,
        "traced_current_bytes": current_bytes,
        "traced_peak_bytes": peak_bytes,
        "net_allocated_blocks": sum(
            stat.count_diff for stat in diff if stat.count_diff > 0
        ),
        "net_allocated_bytes": sum(
            stat.size_diff for stat in diff if stat.size_diff > 0
        ),
    }


def collect_tool_facts(plan: NativeLinkPlan) -> dict[str, object]:
    facts = native_link_tool_facts(plan)
    identity = [
        {key: fact[key] for key in ("role", "resolved", "version", "sha256")}
        for fact in facts
    ]
    return {"tools": facts, "fingerprint": _stable_hash(identity)}


def _child_cpu_snapshot() -> tuple[int, int] | None:
    if os.name == "nt":
        return None
    try:
        import resource

        usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    except (ImportError, OSError):
        return None
    return (int(usage.ru_utime * 1_000_000_000), int(usage.ru_stime * 1_000_000_000))


def measure_command(
    command: Sequence[str],
    *,
    cwd: Path,
    timeout: float,
    env: Mapping[str, str] | None = None,
) -> tuple[harness_memory_guard.GuardedCompletedProcess, dict[str, object]]:
    cpu_before = _child_cpu_snapshot()
    started = time.perf_counter_ns()
    result = harness_memory_guard.guarded_completed_process(
        list(command),
        prefix="MOLT_LINK_BENCH",
        cwd=cwd,
        env=env,
        capture_output=True,
        timeout=timeout,
        limits=harness_memory_guard.limits_from_env("MOLT_LINK_BENCH", env),
    )
    orchestration_wall_ns = time.perf_counter_ns() - started
    child_wall_ns = (
        int(result.elapsed_s * 1_000_000_000)
        if result.elapsed_s is not None
        else orchestration_wall_ns
    )
    cpu_after = _child_cpu_snapshot()
    cpu_user_ns: int | None = None
    cpu_system_ns: int | None = None
    if cpu_before is not None and cpu_after is not None:
        cpu_user_ns = max(0, cpu_after[0] - cpu_before[0])
        cpu_system_ns = max(0, cpu_after[1] - cpu_before[1])
    peak_process = result.peak.rss_kb * 1024 if result.peak is not None else None
    peak_tree = (
        result.peak_total.rss_kb * 1024 if result.peak_total is not None else None
    )
    return result, {
        "wall_ns": child_wall_ns,
        "orchestration_wall_ns": orchestration_wall_ns,
        "cpu_user_ns": cpu_user_ns,
        "cpu_system_ns": cpu_system_ns,
        "cpu_source": "getrusage_children" if cpu_before is not None else "unavailable",
        "peak_process_rss_bytes": peak_process,
        "peak_tree_rss_bytes": peak_tree,
        "returncode": result.returncode,
        "timed_out": result.timed_out,
    }


def _measure_finalization(action: Callable[[], str | None]) -> dict[str, object]:
    wall_started = time.perf_counter_ns()
    cpu_started = time.process_time_ns()
    error = action()
    return {
        "wall_ns": time.perf_counter_ns() - wall_started,
        "parent_cpu_ns": time.process_time_ns() - cpu_started,
        "error": error,
    }


def _read_bolt_telemetry(path: Path) -> dict[str, object]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise LinkBenchmarkError(f"invalid BOLT telemetry {path}: {exc}") from exc
    required = {
        "schema_version",
        "instrument_wall_ns",
        "train_wall_ns",
        "merge_wall_ns",
        "optimize_wall_ns",
        "profile_fragment_count",
        "profile_fragment_bytes",
    }
    missing = required - set(payload)
    if missing or payload.get("schema_version") != 1:
        raise LinkBenchmarkError(
            f"invalid BOLT telemetry contract: missing={sorted(missing)} "
            f"schema={payload.get('schema_version')!r}"
        )
    return payload


def _count_llvm_readobj_records(text: str) -> dict[str, int]:
    context: str | None = None
    counts: Counter[str] = Counter()
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if line in {"Sections [", "Symbols [", "Relocations ["}:
            context = line.split()[0].lower()
            continue
        if raw_line == "]":
            context = None
            continue
        if context == "sections" and raw_line.startswith("  Section {"):
            counts["sections"] += 1
        elif context == "symbols" and raw_line.startswith("  Symbol {"):
            counts["symbols"] += 1
        elif context == "relocations" and raw_line.startswith("    0x"):
            counts["relocations"] += 1
    return {name: counts[name] for name in ("symbols", "sections", "relocations")}


def inspect_binary(path: Path, tool_facts: Mapping[str, object]) -> dict[str, object]:
    inspector = next(
        (
            fact
            for fact in tool_facts.get("tools", [])
            if isinstance(fact, Mapping)
            and fact.get("role") == "inspector"
            and fact.get("resolved") is True
        ),
        None,
    )
    base: dict[str, object] = {
        "path": str(path.resolve()),
        "size_bytes": path.stat().st_size,
        "sha256": _sha256_file(path),
        "symbols": None,
        "sections": None,
        "relocations": None,
        "inspector": None,
    }
    if not isinstance(inspector, Mapping) or not isinstance(inspector.get("path"), str):
        base["inspection_status"] = "llvm-readobj-unavailable"
        return base
    command = [
        str(inspector["path"]),
        "--sections",
        "--symbols",
        "--relocations",
        str(path),
    ]
    result = _COMMANDS.run(
        command,
        cwd=path.parent,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=30,
        check=False,
    )
    base["inspector"] = str(inspector["path"])
    if result.returncode != 0:
        base["inspection_status"] = f"failed:{result.returncode}"
        return base
    base.update(_count_llvm_readobj_records(result.stdout))
    base["inspection_status"] = "ok"
    return base


def summarize_runs(runs: Sequence[Mapping[str, object]]) -> dict[str, object]:
    summary: dict[str, object] = {}
    for phase in ("cold_first", "warm", "relink"):
        selected = [run for run in runs if run.get("phase") == phase]
        if not selected:
            continue
        walls = [int(run["execution"]["wall_ns"]) for run in selected]  # type: ignore[index]
        orchestration_walls = [
            int(run["execution"]["orchestration_wall_ns"])
            for run in selected  # type: ignore[index]
        ]
        finalization = [
            int(finalize["wall_ns"])
            for run in selected
            if isinstance((finalize := run.get("finalization")), Mapping)
        ]
        tree_rss = [
            int(value)
            for run in selected
            if (value := run["execution"].get("peak_tree_rss_bytes")) is not None  # type: ignore[union-attr,index]
        ]
        median_wall = int(statistics.median(walls))
        mad_wall = int(statistics.median(abs(value - median_wall) for value in walls))
        relative_mad = mad_wall / median_wall if median_wall else None
        summary[phase] = {
            "runs": len(selected),
            "link_wall_ns_median": median_wall,
            "link_wall_ns_mad": mad_wall,
            "link_wall_relative_mad": relative_mad,
            "stable": len(selected) >= 5
            and relative_mad is not None
            and relative_mad <= 0.05,
            "link_wall_ns_min": min(walls),
            "link_wall_ns_max": max(walls),
            "orchestration_wall_ns_median": int(statistics.median(orchestration_walls)),
            "finalize_wall_ns_median": (
                int(statistics.median(finalization)) if finalization else None
            ),
            "peak_tree_rss_bytes_max": max(tree_rss, default=None),
        }
    return summary


def _quiescence_ok(report: Mapping[str, object]) -> bool:
    quiescence = report.get("quiescence")
    if not isinstance(quiescence, Mapping):
        return False
    return all(
        isinstance(sample, Mapping)
        and sample.get("certified") is True
        and sample.get("competing_builds") == 0
        for sample in (quiescence.get("before"), quiescence.get("after"))
    )


def report_attestation(report: Mapping[str, object]) -> dict[str, object]:
    plan_metrics = report.get("plan_metrics")
    plan_stable = (
        isinstance(plan_metrics, Mapping) and plan_metrics.get("stable") is True
    )
    quiet = _quiescence_ok(report)
    summary = report.get("summary")
    warm = summary.get("warm") if isinstance(summary, Mapping) else None
    link_stable = isinstance(warm, Mapping) and warm.get("stable") is True
    return {
        "quiescence_certified": quiet,
        "plan_stable": plan_stable,
        "plan_attestable": quiet and plan_stable,
        "link_stable": link_stable,
        "link_attestable": quiet and link_stable,
    }


def comparison_identity(
    *,
    host: Mapping[str, object],
    plan_payload: Mapping[str, object],
    inputs: Mapping[str, object],
    tools: Mapping[str, object],
    warm_runs: int,
    bolt_training_command: str | None,
    measurement_mode: str,
) -> dict[str, str]:
    identity = {
        "host_fingerprint": str(host["fingerprint"]),
        "plan_fingerprint": _stable_hash(plan_payload),
        "input_fingerprint": str(inputs["fingerprint"]),
        "tool_fingerprint": str(tools["fingerprint"]),
        "measurement_fingerprint": measurement_authority_fingerprint(),
    }
    identity["comparison_fingerprint"] = _stable_hash(
        {
            **identity,
            "warm_runs": warm_runs,
            "bolt_training_command": bolt_training_command,
            "measurement_mode": measurement_mode,
        }
    )
    return identity


def compare_reports(
    baseline: Mapping[str, object], current: Mapping[str, object]
) -> dict[str, object]:
    baseline_identity = baseline.get("identity")
    current_identity = current.get("identity")
    if not isinstance(baseline_identity, Mapping) or not isinstance(
        current_identity, Mapping
    ):
        raise LinkBenchmarkError("reports must contain identity objects")
    drift = [
        field
        for field in IDENTITY_FIELDS
        if baseline_identity.get(field) != current_identity.get(field)
    ]
    if drift:
        raise LinkBenchmarkError(
            "refusing native-link comparison across drifted identity: "
            + ", ".join(drift)
        )
    baseline_summary = baseline.get("summary")
    current_summary = current.get("summary")
    if not isinstance(baseline_summary, Mapping) or not isinstance(
        current_summary, Mapping
    ):
        raise LinkBenchmarkError("reports must contain run summaries")
    phases: dict[str, object] = {}
    for phase in ("cold_first", "warm", "relink"):
        old = baseline_summary.get(phase)
        new = current_summary.get(phase)
        if not isinstance(old, Mapping) or not isinstance(new, Mapping):
            continue
        old_wall = int(old["link_wall_ns_median"])
        new_wall = int(new["link_wall_ns_median"])
        phases[phase] = {
            "link_wall_ratio": new_wall / old_wall if old_wall else None,
            "link_wall_delta_ns": new_wall - old_wall,
            "peak_tree_rss_delta_bytes": (
                None
                if old.get("peak_tree_rss_bytes_max") is None
                or new.get("peak_tree_rss_bytes_max") is None
                else int(new["peak_tree_rss_bytes_max"])
                - int(old["peak_tree_rss_bytes_max"])
            ),
        }
    old_warm = baseline_summary.get("warm")
    new_warm = current_summary.get("warm")
    quiet = _quiescence_ok(baseline) and _quiescence_ok(current)
    stable = (
        isinstance(old_warm, Mapping)
        and isinstance(new_warm, Mapping)
        and old_warm.get("stable") is True
        and new_warm.get("stable") is True
        and quiet
    )
    old_plan = baseline.get("plan_metrics")
    new_plan = current.get("plan_metrics")
    plan_stable = (
        isinstance(old_plan, Mapping)
        and isinstance(new_plan, Mapping)
        and old_plan.get("stable") is True
        and new_plan.get("stable") is True
        and quiet
    )
    plan_comparison = None
    if isinstance(old_plan, Mapping) and isinstance(new_plan, Mapping):
        old_plan_wall = int(old_plan["warm_wall_ns_median"])
        new_plan_wall = int(new_plan["warm_wall_ns_median"])
        plan_comparison = {
            "wall_ratio": new_plan_wall / old_plan_wall if old_plan_wall else None,
            "wall_delta_ns": new_plan_wall - old_plan_wall,
        }
    baseline_implementation = baseline.get("implementation")
    current_implementation = current.get("implementation")
    baseline_implementation_fingerprint = (
        baseline_implementation.get("fingerprint")
        if isinstance(baseline_implementation, Mapping)
        else None
    )
    current_implementation_fingerprint = (
        current_implementation.get("fingerprint")
        if isinstance(current_implementation, Mapping)
        else None
    )
    return {
        "identity": str(current_identity["comparison_fingerprint"]),
        "plan": plan_comparison,
        "plan_attestable": plan_stable,
        "implementation": {
            "baseline_fingerprint": baseline_implementation_fingerprint,
            "current_fingerprint": current_implementation_fingerprint,
            "changed": (
                baseline_implementation_fingerprint
                != current_implementation_fingerprint
            ),
            "baseline_variant": baseline.get("variant"),
            "current_variant": current.get("variant"),
        },
        "phases": phases,
        "attestable": stable,
        "attestation_reason": (
            "matching identity, certified quiescence, and both warm samples "
            "satisfy n>=5, relative MAD<=5%"
            if stable
            else "descriptive only: requires certified quiescence with no competing "
            "builds and both warm samples n>=5, relative MAD<=5%"
        ),
    }


def validate_report(report: Mapping[str, object], *, require_runs: bool = True) -> None:
    errors: list[str] = []
    if report.get("schema_version") != SCHEMA_VERSION or report.get("kind") != KIND:
        errors.append("unsupported schema/kind")
    target = report.get("target")
    if not isinstance(target, Mapping):
        errors.append("missing target")
    else:
        if target.get("os") not in SUPPORTED_OSES:
            errors.append(f"unsupported target os {target.get('os')!r}")
        if target.get("arch") not in SUPPORTED_ARCHES:
            errors.append(f"unsupported target arch {target.get('arch')!r}")
    identity = report.get("identity")
    if not isinstance(identity, Mapping):
        errors.append("missing identity")
    else:
        errors.extend(field for field in IDENTITY_FIELDS if not identity.get(field))
    inputs = report.get("inputs")
    if not isinstance(inputs, Mapping) or int(inputs.get("count", 0)) < 3:
        errors.append("benchmark must fingerprint output/stub/runtime inputs")
    plan_metrics = report.get("plan_metrics")
    for field in (
        "wall_ns",
        "cold_wall_ns",
        "warm_wall_ns_median",
        "warm_wall_ns_mad",
        "traced_peak_bytes",
        "net_allocated_blocks",
        "net_allocated_bytes",
    ):
        if not isinstance(plan_metrics, Mapping) or not isinstance(
            plan_metrics.get(field), int
        ):
            errors.append(f"missing integer plan metric {field}")
    runs = report.get("runs")
    if require_runs and (not isinstance(runs, list) or not runs):
        errors.append("no linker runs recorded")
    if errors:
        raise LinkBenchmarkError("invalid native-link report: " + "; ".join(errors))


def _write_report(path: Path, report: Mapping[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    temporary.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    os.replace(temporary, path)


def run_benchmark(args: argparse.Namespace) -> dict[str, object]:
    quiescence_before = asdict(perf_calibration.measure_quiescence())
    output = Path(args.output).expanduser().resolve(strict=False)
    output.parent.mkdir(parents=True, exist_ok=True)
    inputs: dict[str, Path] = {
        "object": Path(args.object).expanduser().resolve(strict=True),
        "stub": Path(args.stub).expanduser().resolve(strict=True),
        "runtime": Path(args.runtime).expanduser().resolve(strict=True),
    }
    if args.stdlib_object:
        inputs["stdlib"] = Path(args.stdlib_object).expanduser().resolve(strict=True)
    inputs["runtime_link_manifest"] = native_link_dependency_manifest_path(
        inputs["runtime"]
    ).resolve(strict=True)
    runtime_manifest = read_native_link_dependency_manifest(
        inputs["runtime"],
        target_triple=args.target_triple,
        source_root=ROOT,
    )
    manifest_source = runtime_manifest.get("source")
    if not isinstance(manifest_source, Mapping) or not isinstance(
        (source_fingerprint := manifest_source.get("fingerprint")), Mapping
    ):
        raise LinkBenchmarkError(
            "runtime native-link manifest has no source fingerprint"
        )

    def factory() -> NativeLinkPlan:
        return _build_native_link_plan(
            output_obj=inputs["object"],
            stub_path=inputs["stub"],
            runtime_lib=inputs["runtime"],
            output_binary=output,
            target_triple=args.target_triple,
            sysroot_path=Path(args.sysroot) if args.sysroot else None,
            profile=args.profile,
            source_root=ROOT,
            source_fingerprint=source_fingerprint,
            stdlib_obj_path=inputs.get("stdlib"),
            export_molt_runtime_symbols=args.export_runtime_symbols,
            bolt_requested=args.bolt,
        )

    plan, plan_metrics = profile_plan(factory, samples=max(6, args.warm_runs + 1))
    inputs.update(response_file_inputs(plan.command))
    inputs.update(plan_library_inputs(plan.command))
    inputs.update(
        plan_auxiliary_inputs(
            plan.command,
            excluded=(*inputs.values(), output),
        )
    )
    input_facts = collect_input_facts(inputs)
    plan_payload = normalized_plan_payload(plan, inputs=inputs, output=output)
    plan_payload["finalization_policy"] = {
        "ordinary_strip": plan.policy.strip_after_link,
        "bolt_strip": (
            os.environ.get("MOLT_KEEP_SYMBOLS") != "1"
            if plan.policy.bolt_requested
            else None
        ),
    }
    tools = collect_tool_facts(plan)
    host = host_payload()
    identity = comparison_identity(
        host=host,
        plan_payload=plan_payload,
        inputs=input_facts,
        tools=tools,
        warm_runs=args.warm_runs,
        bolt_training_command=args.bolt_training_command,
        measurement_mode="plan_only" if args.plan_only else "full",
    )
    report: dict[str, object] = {
        "schema_version": SCHEMA_VERSION,
        "kind": KIND,
        "status": "ok",
        "created_at_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "hot_path": "native plan -> linker graph/layout/write -> strip/validate/publish",
        "big_o": "O(input_bytes + symbols + sections + relocations + output_bytes)",
        "host": host,
        "quiescence": {
            "policy": (
                "before+after certified; competing_builds=0; "
                "warm n>=5; relative MAD<=5%"
            ),
            "before": quiescence_before,
            "after": None,
        },
        "target": asdict(plan.target),
        "cell": {
            "os": plan.target.os,
            "arch": plan.target.arch,
            "object_format": plan.target.object_format.value,
            "profile": args.profile,
            "linker_plan_kind": plan.capabilities.linker.value,
            "linker_executable": next(
                (
                    Path(str(fact["path"])).name
                    for fact in tools["tools"]
                    if fact["role"] == "linker" and fact["resolved"] is True
                ),
                None,
            ),
            "bolt": args.bolt,
        },
        "identity": identity,
        "plan": plan_payload,
        "plan_metrics": plan_metrics,
        "inputs": input_facts,
        "tools": tools,
        "variant": args.variant,
        "implementation": implementation_source_facts(),
        "runs": [],
    }
    validate_report(report, require_runs=False)

    if args.compare:
        baseline = json.loads(Path(args.compare).read_text(encoding="utf-8"))
        baseline_identity = baseline.get("identity")
        if not isinstance(baseline_identity, Mapping):
            raise LinkBenchmarkError("baseline has no identity")
        drift = [
            field
            for field in IDENTITY_FIELDS
            if baseline_identity.get(field) != identity.get(field)
        ]
        if drift:
            raise LinkBenchmarkError(
                "refusing native-link comparison before execution due to drift: "
                + ", ".join(drift)
            )

    if args.plan_only:
        report["summary"] = {}
        report["output"] = None
        report["quiescence"]["after"] = asdict(  # type: ignore[index]
            perf_calibration.measure_quiescence()
        )
        report["attestation"] = report_attestation(report)
        validate_report(report, require_runs=False)
        if args.compare:
            baseline = json.loads(Path(args.compare).read_text(encoding="utf-8"))
            report["comparison"] = compare_reports(baseline, report)
        return report

    phase_names = ["cold_first", *("warm" for _ in range(args.warm_runs)), "relink"]
    runs: list[dict[str, object]] = []
    for index, phase in enumerate(phase_names):
        candidate = output.with_name(
            f".{output.stem}.{phase}-{index}-{uuid.uuid4().hex}{output.suffix}"
        )
        command = _native_link_execution_command(
            plan.command, planned_output=output, execution_output=candidate
        )
        execution_result, execution = measure_command(
            command, cwd=output.parent, timeout=args.timeout
        )
        run: dict[str, object] = {
            "phase": phase,
            "iteration": index,
            "execution": execution,
            "candidate_size_bytes": candidate.stat().st_size
            if candidate.exists()
            else None,
        }
        if execution_result.returncode != 0 or not candidate.is_file():
            stderr_tail = "\n".join((execution_result.stderr or "").splitlines()[-20:])
            run["stderr_tail"] = stderr_tail
            runs.append(run)
            report["runs"] = runs
            report["summary"] = summarize_runs(runs)
            raise LinkBenchmarkError(
                f"native linker failed in {phase} with rc={execution_result.returncode}: "
                f"{stderr_tail or 'no stderr'}",
                report=report,
            )

        final_candidate = candidate
        if args.bolt:
            telemetry_path = output.parent / f".{candidate.name}.bolt-telemetry.json"
            bolt_script = ROOT / "tools" / "bolt_optimize.sh"
            bolt_command = ["bash", str(bolt_script), str(candidate)]
            if args.bolt_training_command:
                bolt_command.append(args.bolt_training_command)
            bolt_env = dict(os.environ)
            bolt_env["MOLT_BOLT_TELEMETRY_JSON"] = str(telemetry_path)
            bolt_result, bolt_measurement = measure_command(
                bolt_command,
                cwd=output.parent,
                timeout=args.bolt_timeout,
                env=bolt_env,
            )
            run["bolt_total"] = bolt_measurement
            if bolt_result.returncode != 0:
                runs.append(run)
                report["runs"] = runs
                report["summary"] = summarize_runs(runs)
                raise LinkBenchmarkError(
                    f"BOLT failed in {phase} with rc={bolt_result.returncode}",
                    report=report,
                )
            run["bolt"] = _read_bolt_telemetry(telemetry_path)
            final_candidate = Path(f"{candidate}.bolt")
            if not final_candidate.is_file():
                raise LinkBenchmarkError(
                    "BOLT succeeded without its optimized candidate", report=report
                )

        before_finalize = final_candidate.stat().st_size
        finalize_phases: dict[str, int] = {}
        finalization = _measure_finalization(
            lambda: _finalize_native_link_candidate(
                candidate=final_candidate,
                output_binary=output,
                target_triple=args.target_triple,
                strip=(
                    os.environ.get("MOLT_KEEP_SYMBOLS") != "1"
                    if args.bolt
                    else plan.policy.strip_after_link
                ),
                phase_times=finalize_phases,
            )
        )
        finalization.update(finalize_phases)
        run["finalization"] = finalization
        if finalization["error"] is not None:
            runs.append(run)
            report["runs"] = runs
            report["summary"] = summarize_runs(runs)
            raise LinkBenchmarkError(str(finalization["error"]), report=report)
        published_size = output.stat().st_size
        run["published_size_bytes"] = published_size
        run["strip_delta_bytes"] = published_size - before_finalize
        if args.bolt:
            candidate.unlink(missing_ok=True)
        runs.append(run)

    report["runs"] = runs
    report["summary"] = summarize_runs(runs)
    report["output"] = inspect_binary(output, tools)
    report["quiescence"]["after"] = asdict(  # type: ignore[index]
        perf_calibration.measure_quiescence()
    )
    report["attestation"] = report_attestation(report)
    validate_report(report)
    if args.compare:
        baseline = json.loads(Path(args.compare).read_text(encoding="utf-8"))
        report["comparison"] = compare_reports(baseline, report)
    return report


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--object", required=True, help="Molt-produced native object")
    parser.add_argument("--stub", required=True, help="generated native main stub")
    parser.add_argument("--runtime", required=True, help="Molt runtime static library")
    parser.add_argument("--stdlib-object")
    parser.add_argument("--output", required=True, help="published benchmark binary")
    parser.add_argument("--target-triple")
    parser.add_argument("--sysroot")
    parser.add_argument("--profile", choices=("dev", "release"), default="release")
    parser.add_argument("--warm-runs", type=int, default=5)
    parser.add_argument("--timeout", type=float, default=300.0)
    parser.add_argument("--bolt-timeout", type=float, default=600.0)
    parser.add_argument("--bolt", action="store_true")
    parser.add_argument("--bolt-training-command")
    parser.add_argument("--export-runtime-symbols", action="store_true")
    parser.add_argument("--compare", help="drift-compatible baseline report")
    parser.add_argument(
        "--variant",
        default="production",
        help="report-only experimental treatment label; never relaxes drift checks",
    )
    parser.add_argument(
        "--plan-only",
        action="store_true",
        help="measure plan construction/allocations without executing the linker",
    )
    parser.add_argument("--json-out", required=True)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    if args.warm_runs < 1:
        raise SystemExit("--warm-runs must be at least 1")
    if args.bolt_training_command and not args.bolt:
        raise SystemExit("--bolt-training-command requires --bolt")
    try:
        report = run_benchmark(args)
        _write_report(Path(args.json_out), report)
    except LinkBenchmarkError as exc:
        if exc.report is not None:
            exc.report["status"] = "failed"
            exc.report["error"] = str(exc)
            quiescence = exc.report.get("quiescence")
            if isinstance(quiescence, dict):
                quiescence["after"] = asdict(perf_calibration.measure_quiescence())
            _write_report(Path(args.json_out), exc.report)
        print(f"native_link_benchmark: {exc}", file=sys.stderr)
        return 2
    except (OSError, ValueError) as exc:
        print(f"native_link_benchmark: {exc}", file=sys.stderr)
        return 2
    print(
        "native_link_benchmark: RECORDED "
        f"cell={report['cell']} identity={report['identity']['comparison_fingerprint']} "
        f"report={args.json_out}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
