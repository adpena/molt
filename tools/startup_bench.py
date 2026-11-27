#!/usr/bin/env python3
"""Release-only startup benchmark with machine-checkable phase evidence."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import platform
import shutil
import statistics
import sys
import time
from pathlib import Path
from typing import Any
from collections.abc import Mapping, Sequence

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

ROOT = Path(__file__).resolve().parents[1]
RESULTS = ROOT / "bench" / "results"
NODE_PROBE = ROOT / "tools" / "startup_node_probe.js"
WASM_RUNNER = ROOT / "wasm" / "run_wasm.js"
DEFAULT_BUDGET = ROOT / "bench" / "scoreboard" / "startup_budget.json"


from tools import output_startup_size_audit as output_audit  # noqa: E402
from tools.bench_evidence import benchmark_reference_invocation  # noqa: E402
from tools.perf_schema import (  # noqa: E402
    PhaseTimingStats,
    RUNTIME_INIT_TRACE,
    benchmark_python_metadata_problems,
)
from molt.process_environment import ProcessEnvironment  # noqa: E402
from molt.python_environment_identity import PythonIdentityVerifier  # noqa: E402
from molt.python_interpreter import (  # noqa: E402
    PythonInterpreter,
    benchmark_python_metadata,
    parse_python_interpreter_request,
    resolve_benchmark_python,
)
from molt.default_paths import (  # noqa: E402
    executable_environment_value,
    expand_user_path,
)
from molt.wasm_artifact import wasm_runtime_manifest_path  # noqa: E402
from molt.dx import scratch_dir  # noqa: E402
from molt.target_python import TargetPythonVersion, resolve_target_python_for_oracle  # noqa: E402

TMP = scratch_dir(ROOT, "startup_bench")

from tools.command_execution import CommandExecutor  # type: ignore  # noqa: E402

_COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_BENCH")

PROBES = {
    "hello": 'print("hello startup")\n',
    "small_compute": "total = 0\nfor value in range(1_000_000):\n    total += value\nprint(total)\n",
}
PHASE_MARKER = "molt_startup_phases="


def _stamp() -> str:
    return dt.datetime.now(dt.UTC).strftime("%Y%m%dT%H%M%SZ")


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _stats(samples_ms: list[float]) -> PhaseTimingStats:
    return {
        "count": len(samples_ms),
        "median_ms": round(statistics.median(samples_ms), 3) if samples_ms else None,
        "min_ms": round(min(samples_ms), 3) if samples_ms else None,
        "max_ms": round(max(samples_ms), 3) if samples_ms else None,
        "samples_ms": [round(value, 3) for value in samples_ms],
    }


def _measure(
    command: list[str],
    *,
    env: ProcessEnvironment,
    samples: int,
    timeout: float,
    label: str,
) -> dict[str, Any]:
    del label
    values: list[float] = []
    records: list[dict[str, Any]] = []
    for index in range(samples):
        started = time.perf_counter_ns()
        result = _COMMANDS.run(
            command,
            cwd=ROOT,
            env=env,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            check=False,
        )
        elapsed_ms = (time.perf_counter_ns() - started) / 1_000_000
        records.append(
            {
                "index": index,
                "returncode": result.returncode,
                "elapsed_ms": round(elapsed_ms, 3),
                "stdout": result.stdout,
                "stderr": result.stderr,
            }
        )
        if result.returncode == 0:
            values.append(elapsed_ms)
    return {
        "ok": len(values) == samples,
        "command": command,
        "stats": _stats(values),
        "records": records,
    }


def _runtime_phases(records: list[dict[str, Any]]) -> dict[str, Any]:
    phases: dict[str, list[float]] = {}
    for record in records:
        for match in RUNTIME_INIT_TRACE.finditer(str(record.get("stderr", ""))):
            phases.setdefault(match.group(3), []).append(int(match.group(2)) / 1000.0)
    medians = {
        name: round(statistics.median(values), 4) for name, values in phases.items()
    }
    return {
        "phase_median_ms": medians,
        "total_median_ms": round(sum(medians.values()), 4) if medians else None,
    }


def _parse_node_phases(stderr: str) -> dict[str, Any] | None:
    for line in reversed(stderr.splitlines()):
        if line.startswith(PHASE_MARKER):
            return json.loads(line[len(PHASE_MARKER) :])
    return None


def _phase_stats(
    records: Sequence[Mapping[str, object]],
) -> dict[str, PhaseTimingStats]:
    samples: dict[str, list[float]] = {}
    for record in records:
        phase = _parse_node_phases(str(record.get("stderr", "")))
        if not phase:
            continue
        samples.setdefault("preload_to_exit_ms", []).append(
            float(phase["preload_to_exit_ms"])
        )
        if phase.get("node_to_preload_ms") is not None:
            samples.setdefault("node_to_preload_ms", []).append(
                float(phase["node_to_preload_ms"])
            )
        for name, duration in phase.get("runner_phases_ms", {}).items():
            samples.setdefault(f"runner:{name}", []).append(float(duration))
        if phase.get("first_stdout_ms") is not None:
            samples.setdefault("first_stdout_ms", []).append(
                float(phase["first_stdout_ms"])
            )
        for read in phase.get("reads", []):
            if str(read.get("path", "")).endswith(".wasm"):
                samples.setdefault(f"read:{Path(str(read['path'])).name}", []).append(
                    float(read["duration_ms"])
                )
        for item in phase.get("instantiations", []):
            samples.setdefault(
                f"instantiate:{Path(str(item.get('source', '<bytes>'))).name}", []
            ).append(float(item["duration_ms"]))
    return {name: _stats(values) for name, values in sorted(samples.items())}


def _artifact(path: Path) -> dict[str, Any]:
    return {"path": str(path), "bytes": path.stat().st_size, "sha256": _sha256(path)}


def _build(
    case: output_audit.MatrixCase,
    script: Path,
    out_dir: Path,
    env: ProcessEnvironment,
    timeout: float,
    *,
    target_python: TargetPythonVersion,
) -> output_audit.BuildResult:
    result = output_audit._build_molt_artifact(
        case=case,
        script=script,
        out_dir=out_dir,
        env=env,
        timeout=timeout,
        extra_molt_args=["--python-version", target_python.short],
    )
    if result.returncode != 0:
        raise RuntimeError(f"build failed for {case.id}: {result.stderr}")
    return result


def _node_command(artifact: Path) -> list[str]:
    node = shutil.which("node")
    if not node:
        raise RuntimeError("node is required for wasm startup measurement")
    return [
        node,
        "--require",
        str(NODE_PROBE),
        str(WASM_RUNNER),
        str(wasm_runtime_manifest_path(artifact)),
    ]


def _baseline_python(env: ProcessEnvironment) -> PythonInterpreter:
    selected = executable_environment_value(env, "MOLT_STARTUP_PYTHON")
    present = "MOLT_STARTUP_PYTHON" in env
    if present and (not selected or "\0" in selected):
        raise ValueError("MOLT_STARTUP_PYTHON must name a CPython executable")
    override = None
    if present:
        path = expand_user_path(selected, environment=env)
        override = str(path if path.is_absolute() else ROOT / path)
    with PythonIdentityVerifier() as verifier:
        return resolve_benchmark_python(
            override=override, verifier=verifier, env=env, cwd=ROOT
        )


def _measure_probe(
    name: str,
    script: Path,
    *,
    env: ProcessEnvironment,
    baseline: PythonInterpreter,
    target_python: TargetPythonVersion,
    samples: int,
    timeout: float,
    build_timeout: float,
) -> dict[str, Any]:
    probe_root = TMP / name
    reference = benchmark_reference_invocation(baseline.command, script, env, cwd=ROOT)
    cpython = _measure(
        list(reference.command),
        env=reference.environment,
        samples=samples,
        timeout=timeout,
        label=f"{name} cpython",
    )
    cpython["importtime"] = _measure(
        [*baseline.command, "-X", "importtime", str(script)],
        env=reference.environment,
        samples=1,
        timeout=timeout,
        label=f"{name} cpython importtime",
    )
    row: dict[str, Any] = {"probe": name, "source": str(script), "cpython": cpython}
    try:
        native = _build(
            output_audit.MatrixCase(
                "native", "release", "auto", stdlib_profile="micro"
            ),
            script,
            probe_root / "native",
            env,
            build_timeout,
            target_python=target_python,
        )
        wasm = _build(
            output_audit.MatrixCase(
                "wasm",
                "release",
                "auto",
                stdlib_profile="micro",
                linked=True,
                require_linked=True,
            ),
            script,
            probe_root / "wasm",
            env,
            build_timeout,
            target_python=target_python,
        )
    except Exception as exc:
        row["build_blocker"] = {"type": type(exc).__name__, "message": str(exc)}
        return row
    trace_env = env.updated({"MOLT_TRACE_RUNTIME_INIT": "1"})
    native_run = _measure(
        [str(native.artifact)],
        env=trace_env,
        samples=samples,
        timeout=timeout,
        label=f"{name} native",
    )
    native_run["runtime_init"] = _runtime_phases(native_run["records"])
    wasm_linked = _measure(
        _node_command(wasm.artifact),
        env=env,
        samples=samples,
        timeout=timeout,
        label=f"{name} wasm-linked",
    )
    wasm_linked["phases"] = _phase_stats(wasm_linked["records"])
    row.update(
        {
            "native": {
                "build_command": native.command,
                "artifact": _artifact(native.artifact),
                "run": native_run,
            },
            "wasm": {
                "build_command": wasm.command,
                "linked_artifact": _artifact(wasm.artifact),
                "linked": wasm_linked,
            },
        }
    )
    return row


def _budget_status(
    report: dict[str, Any], budget_path: Path, strict: bool
) -> dict[str, Any]:
    policy = json.loads(budget_path.read_text(encoding="utf-8"))
    hello = next(row for row in report["probes"] if row["probe"] == "hello")
    values = {
        "native_hello_ms": hello["native"]["run"]["stats"]["median_ms"],
        "cpython_hello_ms": hello["cpython"]["stats"]["median_ms"],
        "wasm_linked_hello_ms": hello["wasm"]["linked"]["stats"]["median_ms"],
    }
    checks = []
    for name, rule in policy["budgets"].items():
        measured = values.get(name)
        limit = rule.get("max_ms")
        passed = measured is not None and (limit is None or measured <= limit)
        checks.append(
            {
                "name": name,
                "measured_ms": measured,
                "max_ms": limit,
                "passed": passed,
                "mode": rule["mode"],
            }
        )
    failed = [
        item
        for item in checks
        if not item["passed"] and (strict or item["mode"] == "strict")
    ]
    return {
        "policy": str(budget_path),
        "strict_requested": strict,
        "checks": checks,
        "ok": not failed,
    }


def _attestation(report: dict[str, Any], samples: int) -> dict[str, Any]:
    def complete(run: dict[str, Any]) -> bool:
        stats = run.get("stats", {})
        return (
            run.get("ok") is True
            and stats.get("count") == samples
            and stats.get("median_ms") is not None
        )

    medians_present = len(report["probes"]) == len(PROBES) and all(
        complete(row.get("cpython", {}))
        and complete(row.get("native", {}).get("run", {}))
        and complete(row.get("wasm", {}).get("linked", {}))
        for row in report["probes"]
    )
    reasons = benchmark_python_metadata_problems(report.get("baseline_python"))
    if not medians_present or samples < 3:
        reasons.append("complete release medians unavailable")
    accepted = not reasons
    return {
        "grade": "A12-release-median-baseline" if accepted else "refused",
        "release_profile": True,
        "sample_count": samples,
        "median_required": True,
        "serial_execution": True,
        "accepted": accepted,
        "reasons": reasons,
        "variant_ii": "required only for a claimed before/after startup improvement",
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--build-timeout", type=float, default=1800.0)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--budget", type=Path, default=DEFAULT_BUDGET)
    parser.add_argument("--strict", action="store_true")
    args = parser.parse_args()
    if args.samples < 3:
        parser.error("A12 requires at least 3 samples")
    env = output_audit._canonical_env(os.environ.copy())
    baseline = _baseline_python(env)
    release = parse_python_interpreter_request(baseline.version)
    target_python = resolve_target_python_for_oracle((release.major, release.minor))
    TMP.mkdir(parents=True, exist_ok=True)
    RESULTS.mkdir(parents=True, exist_ok=True)
    scripts = {}
    for name, source in PROBES.items():
        path = TMP / "probes" / f"{name}.py"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source, encoding="utf-8")
        scripts[name] = path
    node = shutil.which("node")
    node_boot = (
        _measure(
            [node, "-e", ""],
            env=env,
            samples=args.samples,
            timeout=args.timeout,
            label="node boot",
        )
        if node
        else {"ok": False, "skipped": "node unavailable"}
    )
    report: dict[str, Any] = {
        "schema_version": 2,
        "claim": "STARTUP-BASELINE",
        "recorded_at": _stamp(),
        "baseline_python": benchmark_python_metadata(baseline),
        "methodology": {
            "build_profile": "release",
            "target_python": target_python.short,
            "samples": args.samples,
            "statistic": "median",
            "same_machine": True,
            "serial": True,
        },
        "machine": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "processor": platform.processor(),
            "python": sys.version,
            "node": node,
        },
        "node_boot": node_boot,
        "probes": [],
    }
    for name, path in scripts.items():
        report["probes"].append(
            _measure_probe(
                name,
                path,
                env=env,
                baseline=baseline,
                target_python=target_python,
                samples=args.samples,
                timeout=args.timeout,
                build_timeout=args.build_timeout,
            )
        )
    report["build_blockers"] = [
        {"probe": row["probe"], **row["build_blocker"]}
        for row in report["probes"]
        if "build_blocker" in row
    ]
    report["attestation"] = _attestation(report, args.samples)
    complete_hello = any(
        row.get("probe") == "hello" and "native" in row and "wasm" in row
        for row in report["probes"]
    )
    report["budget"] = (
        _budget_status(report, args.budget, args.strict)
        if complete_hello
        else {
            "policy": str(args.budget),
            "ok": False,
            "skipped": "hello release outputs unavailable",
        }
    )
    report["ok"] = report["attestation"]["accepted"] and report["budget"]["ok"]
    output = args.output or RESULTS / f"startup_baseline_{report['recorded_at']}.json"
    output.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(
        json.dumps(
            {"ok": report["ok"], "output": str(output), "claim": report["claim"]},
            sort_keys=True,
        )
    )
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
