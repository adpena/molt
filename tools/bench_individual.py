#!/usr/bin/env python3
"""Individual benchmark runner with warm daemon reuse by default.

Runs benchmarks through the normal Molt developer path so build timings reflect
user-code compilation after the runtime/backend artifacts are warm.  Pass
``--isolate-daemon`` when deliberately measuring cold daemon behavior or
investigating daemon crash cascade failures.

Usage:
    python tools/bench_individual.py
    python tools/bench_individual.py --samples 5 --warmup 1 --json-out results.json
    python tools/bench_individual.py --bench bench_fib.py --bench bench_sum.py
    python tools/bench_individual.py --skip bench_startup.py
"""

from __future__ import annotations

import argparse
from datetime import UTC, datetime
import json
import os
import platform
import re
import statistics
import sys
import tempfile
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = REPO_ROOT / "src"
TOOLS_ROOT = Path(__file__).resolve().parent
BENCH_RESULTS_DIR = REPO_ROOT / "bench" / "results"
BENCH_TMP_ROOT = REPO_ROOT / "tmp" / "bench"
for _import_root in (SRC_ROOT, TOOLS_ROOT):
    if str(_import_root) not in sys.path:
        sys.path.insert(0, str(_import_root))

from bench_metadata import benchmark_reference_contract  # noqa: E402
import harness_memory_guard  # noqa: E402
import bench_suites  # noqa: E402
from molt import backend_daemon_custody as daemon_custody  # noqa: E402
import perf_authority  # noqa: E402

BENCHMARKS = bench_suites.BENCHMARKS
MOLT_ARGS_BY_BENCH = bench_suites.MOLT_ARGS_BY_BENCH
molt_args_for_benchmark = bench_suites.molt_args_for_benchmark


def _guarded_bench_process(
    cmd: list[str],
    *,
    timeout: float | None = None,
) -> harness_memory_guard.GuardedCompletedProcess:
    env = harness_memory_guard.canonical_harness_env(os.environ, repo_root=REPO_ROOT)
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH", env)
    return harness_memory_guard.guarded_completed_process(
        cmd,
        prefix="MOLT_BENCH",
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
        limits=limits,
    )


class RunSample:
    __slots__ = ("elapsed_s", "output")

    def __init__(self, elapsed_s: float, output: str) -> None:
        self.elapsed_s = elapsed_s
        self.output = output


class SampleBatch:
    __slots__ = ("failed_detail", "failed_phase", "ok", "samples", "warmup_samples")

    def __init__(
        self,
        samples: list[RunSample] | None = None,
        warmup_samples: list[RunSample] | None = None,
        ok: bool = False,
        failed_phase: str | None = None,
        failed_detail: str = "",
    ) -> None:
        self.samples = samples if samples is not None else []
        self.warmup_samples = warmup_samples if warmup_samples is not None else []
        self.ok = ok
        self.failed_phase = failed_phase
        self.failed_detail = failed_detail

    @property
    def times_s(self) -> list[float]:
        return [sample.elapsed_s for sample in self.samples]

    @property
    def warmup_times_s(self) -> list[float]:
        return [sample.elapsed_s for sample in self.warmup_samples]

    @property
    def first_output(self) -> str:
        return self.samples[0].output if self.samples else ""


# ---------------------------------------------------------------------------
# Daemon management
# ---------------------------------------------------------------------------


class BackendDaemonProcess:
    __slots__ = ("command", "elapsed_sec", "pid", "socket_path")

    def __init__(
        self,
        *,
        pid: int,
        elapsed_sec: int | None,
        socket_path: Path,
        command: str,
    ) -> None:
        self.pid = pid
        self.elapsed_sec = elapsed_sec
        self.socket_path = socket_path
        self.command = command


class BackendDaemonKill:
    __slots__ = ("command", "elapsed_sec", "pid", "reason", "socket_path")

    def __init__(
        self,
        *,
        pid: int,
        socket_path: str,
        reason: str,
        command: str,
        elapsed_sec: int | None,
    ) -> None:
        self.pid = pid
        self.socket_path = socket_path
        self.reason = reason
        self.command = command
        self.elapsed_sec = elapsed_sec

    def to_json(self) -> dict[str, object]:
        return {
            "pid": self.pid,
            "socket_path": self.socket_path,
            "reason": self.reason,
            "command": self.command,
            "elapsed_sec": self.elapsed_sec,
        }


class BackendDaemonCleanupReport:
    __slots__ = (
        "artifact",
        "elapsed_s",
        "killed",
        "killed_at",
        "scanned",
        "session_id",
        "skipped_foreign",
    )

    def __init__(
        self,
        *,
        killed: tuple[BackendDaemonKill, ...],
        scanned: int,
        skipped_foreign: int,
        session_id: str,
        killed_at: str | None,
        elapsed_s: float,
        artifact: str | None,
    ) -> None:
        self.killed = killed
        self.scanned = scanned
        self.skipped_foreign = skipped_foreign
        self.session_id = session_id
        self.killed_at = killed_at
        self.elapsed_s = elapsed_s
        self.artifact = artifact

    @property
    def killed_count(self) -> int:
        return len(self.killed)


def _build_state_root_from_env(env: dict[str, str]) -> Path:
    return daemon_custody.backend_daemon_build_state_root_from_env(
        env,
        project_root=REPO_ROOT,
    )


def _current_session_identity_files(
    env: dict[str, str],
) -> dict[int, daemon_custody.BackendDaemonIdentityRecord]:
    return {
        record.identity.pid: record
        for record in daemon_custody.current_session_backend_daemon_identity_records(
            env,
            project_root=REPO_ROOT,
        )
    }


def _list_backend_daemon_processes() -> list[BackendDaemonProcess]:
    if os.name != "posix":
        return []
    try:
        result = _guarded_bench_process(
            ["ps", "-axo", "pid=,etimes=,command="],
            timeout=10,
        )
    except OSError:
        return []

    processes: list[BackendDaemonProcess] = []
    pattern = re.compile(r"^\s*(\d+)\s+(\d+|-)\s+(.*)$")
    socket_pat = re.compile(r"--socket\s+(\S+)")
    for line in result.stdout.splitlines():
        match = pattern.match(line)
        if match is None:
            continue
        pid = int(match.group(1))
        elapsed_raw = match.group(2)
        cmd = match.group(3)
        if "molt-backend" not in cmd or "--daemon" not in cmd:
            continue
        socket_match = socket_pat.search(cmd)
        if socket_match is None:
            continue
        socket_path = Path(socket_match.group(1)).expanduser()
        processes.append(
            BackendDaemonProcess(
                pid=pid,
                elapsed_sec=None if elapsed_raw == "-" else int(elapsed_raw),
                socket_path=socket_path,
                command=cmd,
            )
        )
    return processes


def _daemon_cleanup_artifact_path() -> Path:
    return BENCH_TMP_ROOT / "daemon_custody.jsonl"


def _record_daemon_cleanup(
    *,
    report: BackendDaemonCleanupReport,
    env: dict[str, str],
) -> str | None:
    if not report.killed:
        return None
    path = _daemon_cleanup_artifact_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "event": "bench_individual_backend_daemon_cleanup",
        "killed_at": report.killed_at,
        "elapsed_s": report.elapsed_s,
        "session_id": report.session_id,
        "scanned": report.scanned,
        "skipped_foreign": report.skipped_foreign,
        "killed": [kill.to_json() for kill in report.killed],
        "target_dir": env.get("CARGO_TARGET_DIR"),
        "build_state_root": str(_build_state_root_from_env(env)),
    }
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(payload, sort_keys=True) + "\n")
    return str(path)


def _cleanup_current_session_backend_daemons(
    *, quiet: bool = False
) -> BackendDaemonCleanupReport:
    """Terminate backend daemons owned by this benchmark session only."""
    started = time.perf_counter()
    env = harness_memory_guard.canonical_harness_env(os.environ, repo_root=REPO_ROOT)
    session_id = env.get("MOLT_SESSION_ID", "")
    owned_identity_files = _current_session_identity_files(env)
    processes = _list_backend_daemon_processes()
    processes_by_pid = {process.pid: process for process in processes}

    killed: list[BackendDaemonKill] = []
    skipped_foreign = 0
    for process in processes:
        identity_record = owned_identity_files.get(process.pid)
        if identity_record is None:
            skipped_foreign += 1
            continue
        identity = identity_record.identity
        if not daemon_custody.backend_daemon_command_matches_identity(
            process.command,
            backend_bin=identity.backend_bin,
            socket_path=identity.socket_path,
        ):
            skipped_foreign += 1
    for identity_record in owned_identity_files.values():
        identity = identity_record.identity
        process = processes_by_pid.get(identity.pid)
        reason = "session_identity_verified"
        if not identity.socket_path.exists():
            reason = f"{reason}:missing_socket"
        if not daemon_custody.terminate_backend_daemon_identity(
            identity,
            grace=0.75,
        ):
            continue
        killed.append(
            BackendDaemonKill(
                pid=identity.pid,
                socket_path=str(identity.socket_path),
                reason=reason,
                command=(
                    process.command
                    if process is not None
                    else identity.command or "<unknown>"
                ),
                elapsed_sec=process.elapsed_sec if process is not None else None,
            )
        )
        daemon_custody.remove_backend_daemon_identity(identity_record.path)

    killed_at = datetime.now(UTC).isoformat() if killed else None
    report = BackendDaemonCleanupReport(
        killed=tuple(killed),
        scanned=len(processes),
        skipped_foreign=skipped_foreign,
        session_id=session_id,
        killed_at=killed_at,
        elapsed_s=round(time.perf_counter() - started, 4),
        artifact=None,
    )
    artifact = _record_daemon_cleanup(report=report, env=env)
    report = BackendDaemonCleanupReport(
        killed=report.killed,
        scanned=report.scanned,
        skipped_foreign=report.skipped_foreign,
        session_id=report.session_id,
        killed_at=report.killed_at,
        elapsed_s=report.elapsed_s,
        artifact=artifact,
    )
    if killed and not quiet:
        pids = ",".join(str(item.pid) for item in killed)
        sockets = ",".join(item.socket_path for item in killed)
        print(
            "  [cleanup] terminated current-session molt-backend daemon(s): "
            f"killed={len(killed)} pids={pids} sockets={sockets} "
            f"session={session_id} killed_at={killed_at} "
            f"elapsed={report.elapsed_s:.2f}s skipped_foreign={skipped_foreign} "
            f"artifact={artifact}",
            file=sys.stderr,
        )
        print(
            "  [cleanup] reason=session identity verification for cold benchmark custody; "
            "next action: if this was unexpected, inspect the daemon custody "
            "artifact and ensure each concurrent agent has a unique MOLT_SESSION_ID.",
            file=sys.stderr,
        )
    return report


def _ensure_clean_slate(quiet: bool = False) -> None:
    """Terminate only this benchmark session's backend daemons."""
    report = _cleanup_current_session_backend_daemons(quiet=quiet)
    # Give the OS time to release sockets and clean up
    if report.killed:
        time.sleep(2)


def _git_rev() -> str | None:
    try:
        res = _guarded_bench_process(
            ["git", "rev-parse", "HEAD"],
            timeout=10,
        )
    except OSError:
        return None
    if res.returncode != 0:
        return None
    return res.stdout.strip() or None


# ---------------------------------------------------------------------------
# Build helpers
# ---------------------------------------------------------------------------


def _molt_build_cmd() -> list[str]:
    """Command prefix for the Molt compiler via uv."""
    return ["uv", "run", "--python", "3.12", "python3"]


def _resolve_molt_output(payload: dict) -> Path | None:
    output_str = payload.get("data", {}).get("output") or payload.get("output")
    if not output_str:
        return None
    output_path = Path(output_str)
    if output_path.exists():
        return output_path
    fallback = output_path.with_suffix(".exe")
    if fallback.exists():
        return fallback
    return None


def molt_build(
    script: str,
    out_dir: Path,
    timeout_s: float,
    extra_args: list[str] | None = None,
    limits: harness_memory_guard.HarnessMemoryLimits | None = None,
) -> tuple[Path | None, float, str]:
    """Build a benchmark with Molt.

    Returns (binary_path_or_None, build_time_seconds, error_message).
    """
    env = harness_memory_guard.canonical_harness_env(os.environ, repo_root=REPO_ROOT)
    env["PYTHONPATH"] = str(REPO_ROOT / "src")

    args = [
        *_molt_build_cmd(),
        "-m",
        "molt.cli",
        "build",
        "--trusted",
        "--json",
        "--out-dir",
        str(out_dir),
    ]
    if extra_args:
        args.extend(extra_args)
    args.append(script)

    start = time.perf_counter()
    res = harness_memory_guard.guarded_completed_process(
        args,
        prefix="MOLT_BENCH",
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout_s,
        limits=limits,
    )
    if res.returncode == harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE:
        return None, time.perf_counter() - start, f"build timed out after {timeout_s}s"
    build_s = time.perf_counter() - start

    if res.returncode != 0:
        err = (res.stderr or res.stdout or "").strip()[:500]
        return None, build_s, f"build failed (rc={res.returncode}): {err}"

    try:
        payload = json.loads(res.stdout.strip() or "{}")
    except json.JSONDecodeError:
        return None, build_s, "build produced invalid JSON"

    binary = _resolve_molt_output(payload)
    if binary is None:
        return None, build_s, "build succeeded but no output binary found"

    return binary, build_s, ""


# ---------------------------------------------------------------------------
# Run helpers
# ---------------------------------------------------------------------------


def run_binary(
    binary: Path,
    timeout_s: float,
    *,
    limits: harness_memory_guard.HarnessMemoryLimits | None = None,
) -> tuple[bool, float, str]:
    """Run a compiled binary.  Returns (ok, elapsed_s, stdout_or_diagnostic)."""
    start = time.perf_counter()
    res = harness_memory_guard.guarded_completed_process(
        [str(binary)],
        prefix="MOLT_BENCH",
        capture_output=True,
        text=True,
        timeout=timeout_s,
        limits=limits,
    )
    if res.returncode == harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE:
        return False, time.perf_counter() - start, f"timed out after {timeout_s}s"
    elapsed = getattr(res, "elapsed_s", None)
    if elapsed is None:
        elapsed = time.perf_counter() - start
    if res.returncode != 0:
        return (
            False,
            elapsed,
            _format_run_failure(res.returncode, res.stdout, res.stderr),
        )
    return True, elapsed, (res.stdout or "").strip()


def run_cpython(
    script: str,
    timeout_s: float,
    *,
    limits: harness_memory_guard.HarnessMemoryLimits | None = None,
) -> tuple[bool, float, str]:
    """Run a script with CPython.  Returns (ok, elapsed_s, stdout_or_diagnostic)."""
    start = time.perf_counter()
    res = harness_memory_guard.guarded_completed_process(
        [sys.executable, script],
        prefix="MOLT_BENCH",
        capture_output=True,
        text=True,
        timeout=timeout_s,
        limits=limits,
    )
    if res.returncode == harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE:
        return False, time.perf_counter() - start, f"timed out after {timeout_s}s"
    elapsed = getattr(res, "elapsed_s", None)
    if elapsed is None:
        elapsed = time.perf_counter() - start
    if res.returncode != 0:
        return (
            False,
            elapsed,
            _format_run_failure(res.returncode, res.stdout, res.stderr),
        )
    return True, elapsed, (res.stdout or "").strip()


def _format_run_failure(returncode: int, stdout: str | None, stderr: str | None) -> str:
    parts = [f"rc={returncode}"]
    if stderr:
        parts.append("stderr:\n" + stderr.strip())
    if stdout:
        parts.append("stdout:\n" + stdout.strip())
    return "\n".join(parts)[:4000]


def collect_samples(
    measure_fn,
    samples: int,
    warmup: int,
) -> SampleBatch:
    """Collect warmup and measured samples, failing closed on any bad sample."""
    warmup_samples: list[RunSample] = []
    for idx in range(warmup):
        ok, elapsed, output = measure_fn()
        if not ok:
            return SampleBatch(
                samples=[],
                warmup_samples=warmup_samples,
                ok=False,
                failed_phase=f"warmup {idx + 1}/{warmup}",
                failed_detail=output,
            )
        warmup_samples.append(RunSample(elapsed_s=elapsed, output=output))

    measured: list[RunSample] = []
    for idx in range(samples):
        ok, elapsed, output = measure_fn()
        if not ok:
            return SampleBatch(
                samples=measured,
                warmup_samples=warmup_samples,
                ok=False,
                failed_phase=f"sample {idx + 1}/{samples}",
                failed_detail=output,
            )
        measured.append(RunSample(elapsed_s=elapsed, output=output))

    return SampleBatch(
        samples=measured,
        warmup_samples=warmup_samples,
        ok=bool(measured),
    )


# ---------------------------------------------------------------------------
# Single benchmark
# ---------------------------------------------------------------------------


def bench_one(
    script: str,
    samples: int,
    warmup: int,
    timeout_build: float,
    timeout_run: float,
    *,
    isolate_daemon: bool = False,
    limits: harness_memory_guard.HarnessMemoryLimits | None = None,
) -> dict:
    """Run a single benchmark.

    Returns a result dict for the JSON report.
    """
    extra_args = molt_args_for_benchmark(script)
    reference_contract = benchmark_reference_contract(script)

    result: dict = {
        "reference_runtime": reference_contract.reference_runtime,
        "reference_reason": reference_contract.reason,
        "build_ok": False,
        "build_time_s": None,
        "run_ok": False,
        "molt_time_s": None,
        "cpython_time_s": None,
        "molt_samples_s": [],
        "molt_warmup_samples_s": [],
        "cpython_samples_s": None,
        "cpython_warmup_samples_s": None,
        "molt_build_s": None,
        "molt_speedup": None,
        "molt_cpython_ratio": None,
        "molt_ok": False,
        "speedup": None,
        "output_match": None,
        "molt_output": None,
        "cpython_output": None,
        "molt_failure_detail": None,
        "cpython_failure_detail": None,
        "error": None,
    }

    if isolate_daemon:
        _ensure_clean_slate()

    # --- Build with Molt ---
    BENCH_TMP_ROOT.mkdir(parents=True, exist_ok=True)
    tmp = tempfile.TemporaryDirectory(prefix="molt-iso-bench-", dir=BENCH_TMP_ROOT)
    out_dir = Path(tmp.name)
    binary, build_s, build_err = molt_build(
        script,
        out_dir,
        timeout_build,
        extra_args=extra_args,
        limits=limits,
    )
    result["build_time_s"] = round(build_s, 4)

    if binary is None:
        result["error"] = build_err
        print(f"  BUILD FAIL: {build_err}", file=sys.stderr)
        if reference_contract.external_baselines:
            cp_batch = collect_samples(
                lambda: run_cpython(script, timeout_run, limits=limits),
                samples=samples,
                warmup=warmup,
            )
            result["cpython_samples_s"] = cp_batch.times_s
            result["cpython_warmup_samples_s"] = cp_batch.warmup_times_s
            if cp_batch.ok:
                cp_time = statistics.median(cp_batch.times_s)
                result["cpython_time_s"] = round(cp_time, 6)
                result["cpython_output"] = cp_batch.first_output
            else:
                result["cpython_failure_detail"] = cp_batch.failed_detail or None
        tmp.cleanup()
        return result

    result["build_ok"] = True
    result["molt_build_s"] = round(build_s, 4)

    # --- Run Molt (multiple samples, take median) ---
    molt_batch = collect_samples(
        lambda: run_binary(binary, timeout_run, limits=limits),
        samples=samples,
        warmup=warmup,
    )
    result["molt_samples_s"] = molt_batch.times_s
    result["molt_warmup_samples_s"] = molt_batch.warmup_times_s
    molt_output = molt_batch.first_output
    if molt_batch.ok:
        result["run_ok"] = True
        result["molt_time_s"] = round(statistics.median(molt_batch.times_s), 6)
        result["molt_output"] = molt_output
    else:
        result["error"] = f"Molt run failed during {molt_batch.failed_phase}"
        if molt_batch.failed_detail:
            result["molt_failure_detail"] = molt_batch.failed_detail
            result["error"] += f": {molt_batch.failed_detail}"

    if not reference_contract.external_baselines:
        if molt_batch.ok:
            result["molt_ok"] = True
    else:
        # --- Run CPython (multiple samples, take median) ---
        cp_batch = collect_samples(
            lambda: run_cpython(script, timeout_run, limits=limits),
            samples=samples,
            warmup=warmup,
        )
        result["cpython_samples_s"] = cp_batch.times_s
        result["cpython_warmup_samples_s"] = cp_batch.warmup_times_s
        cpython_output = cp_batch.first_output
        if cp_batch.ok:
            result["cpython_time_s"] = round(statistics.median(cp_batch.times_s), 6)
            result["cpython_output"] = cpython_output
        else:
            result["cpython_failure_detail"] = cp_batch.failed_detail or None
            if result["error"] is None:
                result["error"] = f"CPython run failed during {cp_batch.failed_phase}"
                if cp_batch.failed_detail:
                    result["error"] += f": {cp_batch.failed_detail}"

        # --- Output match ---
        if molt_batch.ok and cp_batch.ok:
            result["output_match"] = molt_output == cpython_output
            if result["output_match"]:
                result["molt_ok"] = True
            else:
                result["error"] = "output mismatch"
        elif molt_output or cpython_output:
            result["output_match"] = False

    # --- Speedup ---
    if result["molt_ok"]:
        speedup = perf_authority.signed_ratio_value(
            result["cpython_time_s"],
            result["molt_time_s"],
            direction=perf_authority.RatioDirection.SPEEDUP,
        )
        cpython_ratio_block = perf_authority.signed_ratio(
            result["molt_time_s"],
            result["cpython_time_s"],
            direction=perf_authority.RatioDirection.MOLT_OVER_BASELINE,
        )
        result["speedup"] = round(speedup, 1) if speedup is not None else None
        result["molt_speedup"] = speedup
        result["molt_cpython_ratio"] = cpython_ratio_block["value"]
        result["ratio_directions"] = {
            "molt_speedup": perf_authority.RatioDirection.SPEEDUP.value,
            "molt_cpython_ratio": cpython_ratio_block["direction"],
        }

    tmp.cleanup()
    return result


# ---------------------------------------------------------------------------
# Summary printer
# ---------------------------------------------------------------------------


def print_summary(results: dict[str, dict]) -> None:
    """Print an aligned summary table to stdout."""
    header = f"{'Benchmark':<42} {'Build':>7} {'Molt(s)':>10} {'CPy(s)':>10} {'Speedup':>8} {'Match':>6}"
    sep = "-" * len(header)
    print()
    print(sep)
    print(header)
    print(sep)

    pass_count = 0
    fail_count = 0
    total = len(results)

    for name, r in results.items():
        build_str = "OK" if r["build_ok"] else "FAIL"
        molt_str = f"{r['molt_time_s']:.4f}" if r["molt_time_s"] is not None else "-"
        cpy_str = (
            f"{r['cpython_time_s']:.4f}" if r["cpython_time_s"] is not None else "-"
        )
        speedup_str = f"{r['speedup']:.1f}x" if r["speedup"] is not None else "-"

        if r["output_match"] is True:
            match_str = "YES"
        elif r["output_match"] is False:
            match_str = "NO"
        else:
            match_str = "-"

        if r["molt_ok"]:
            pass_count += 1
        else:
            fail_count += 1

        print(
            f"{name:<42} {build_str:>7} {molt_str:>10} {cpy_str:>10} {speedup_str:>8} {match_str:>6}"
        )

    print(sep)
    print(f"Total: {total}  |  Pass: {pass_count}  |  Fail: {fail_count}")
    print(sep)
    print()


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run Molt benchmarks with warm backend daemon reuse by default.",
    )
    parser.add_argument(
        "--samples",
        type=int,
        default=3,
        help="Number of run samples per benchmark; takes median (default: 3)",
    )
    parser.add_argument(
        "--warmup",
        type=int,
        default=1,
        help="Discarded warmup runs per runtime before measured samples (default: 1)",
    )
    parser.add_argument(
        "--json-out",
        type=str,
        default=None,
        help="Path to write JSON results",
    )
    parser.add_argument(
        "--bench",
        action="append",
        default=None,
        help="Run only specific benchmark(s) by filename (repeatable)",
    )
    parser.add_argument(
        "--skip",
        action="append",
        default=None,
        help="Skip specific benchmark(s) by filename (repeatable)",
    )
    parser.add_argument(
        "--timeout-build",
        type=float,
        default=120,
        help="Build timeout in seconds (default: 120)",
    )
    parser.add_argument(
        "--timeout-run",
        type=float,
        default=60,
        help="Run timeout in seconds (default: 60)",
    )
    parser.add_argument(
        "--isolate-daemon",
        action="store_true",
        help=(
            "Kill molt-backend before each benchmark and at exit. "
            "Use only for cold-start/crash-isolation diagnostics."
        ),
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if args.samples <= 0:
        raise SystemExit("--samples must be >= 1")
    if args.warmup < 0:
        raise SystemExit("--warmup must be >= 0")
    BENCH_RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    BENCH_TMP_ROOT.mkdir(parents=True, exist_ok=True)
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH")

    # Filter benchmarks
    benchmarks = list(BENCHMARKS)

    if args.bench:
        selected = set(args.bench)
        benchmarks = [
            b
            for b in benchmarks
            if Path(b).name in selected or Path(b).stem in selected or b in selected
        ]
        if not benchmarks:
            print(f"No benchmarks matched: {args.bench}", file=sys.stderr)
            sys.exit(1)

    if args.skip:
        skip_set = set(args.skip)
        benchmarks = [
            b
            for b in benchmarks
            if Path(b).name not in skip_set
            and Path(b).stem not in skip_set
            and b not in skip_set
        ]

    total = len(benchmarks)
    print(f"Running {total} benchmark(s) with {args.samples} sample(s) each")
    print(f"Warmup: {args.warmup} discarded sample(s) per runtime")
    print(f"Build timeout: {args.timeout_build}s  |  Run timeout: {args.timeout_run}s")
    print(
        "Backend daemon: "
        + ("cold isolated per benchmark" if args.isolate_daemon else "warm reused")
    )
    print()

    results: dict[str, dict] = {}

    with harness_memory_guard.repo_process_sentinel(
        repo_root=REPO_ROOT,
        artifact_root=BENCH_TMP_ROOT,
        label="bench_individual",
        limits=limits,
    ):
        for idx, script in enumerate(benchmarks, 1):
            name = Path(script).name
            print(f"[{idx}/{total}] {name}")
            result = bench_one(
                script,
                samples=args.samples,
                warmup=args.warmup,
                timeout_build=args.timeout_build,
                timeout_run=args.timeout_run,
                isolate_daemon=args.isolate_daemon,
                limits=limits,
            )
            results[name] = result

            # Quick inline status
            if result["build_ok"] and result["run_ok"]:
                speedup = f" ({result['speedup']:.1f}x)" if result["speedup"] else ""
                cpython = (
                    f"{result['cpython_time_s']:.4f}s"
                    if result["cpython_time_s"] is not None
                    else "-"
                )
                print(
                    f"  -> OK  molt={result['molt_time_s']:.4f}s  cpython={cpython}{speedup}"
                )
            elif result["build_ok"]:
                print("  -> BUILD OK, RUN FAIL")
            else:
                print("  -> BUILD FAIL")

    # Summary
    print_summary(results)

    # JSON output
    try:
        load_avg = os.getloadavg()
    except OSError:
        load_avg = None
    report = {
        "schema_version": 1,
        "created_at": datetime.now(UTC).isoformat(),
        "git_rev": _git_rev(),
        "super_run": False,
        "samples": args.samples,
        "warmup": args.warmup,
        "timing_mode": "warm_throughput" if args.warmup > 0 else "cold_first_run",
        "system": {
            "platform": platform.platform(),
            "python": sys.version.split()[0],
            "machine": platform.machine(),
            "cpu_count": os.cpu_count(),
            "load_avg": load_avg,
        },
        "memory_guard": harness_memory_guard.limits_summary(limits),
        "benchmarks": results,
    }
    if args.json_out:
        out_path = Path(args.json_out)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(json.dumps(report, indent=2) + "\n")
        print(f"Results written to {out_path}")
    else:
        # Also dump to a default location
        default_out = BENCH_RESULTS_DIR / (
            "bench_individual_" + datetime.now(UTC).strftime("%Y%m%d_%H%M%S") + ".json"
        )
        default_out.write_text(json.dumps(report, indent=2) + "\n")
        print(f"Results written to {default_out}")

    if args.isolate_daemon:
        _ensure_clean_slate(quiet=True)


if __name__ == "__main__":
    main()
