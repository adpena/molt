#!/usr/bin/env python3
"""Run the IR structure verifier across a suite of Python source files.

Compiler workers emit TIR JSON while the supervisor owns one long-lived Rust
verifier process for structural validation.

Usage:
    python tools/verify_ir_suite.py [--dir DIR] [--glob PATTERN] [--fail-fast] [--quiet]

Exit codes:
    0 — every file is either structurally valid or a typed frontend rejection
    1 — one or more compiled files have IR errors or untyped compile failures
    2 — usage error
"""

import argparse
import ctypes
import multiprocessing
import os
import sys
import time
import traceback
import tomllib
from multiprocessing.connection import Connection, wait as wait_connections
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
POOL_POLICY = ROOT / "config" / "ir_verification_pool.toml"
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.artifact_publish import atomic_write_json  # noqa: E402
from tools.check_ir_structure import verify_tir  # noqa: E402
from tools.proof_counts import fail_closed_proof_exit_code  # noqa: E402
from tools.resource_pressure import plan_resource_pressure  # noqa: E402
from tools.rust_ir_verifier import (  # noqa: E402
    close_process_local_verifier,
    verifier_binary,
)
from molt.compat import CompatibilityError  # noqa: E402


def _load_pool_policy() -> dict[str, object]:
    payload = tomllib.loads(POOL_POLICY.read_text(encoding="utf-8"))
    if payload.get("schema") != "molt.ir-verification-pool.v1":
        raise ValueError("IR verification pool policy schema mismatch")
    policy = payload.get("policy")
    if not isinstance(policy, dict):
        raise ValueError("IR verification pool policy table is missing")
    for field in ("max_workers", "max_cases_per_worker"):
        value = policy.get(field)
        if not isinstance(value, int) or isinstance(value, bool) or value < 1:
            raise ValueError(f"IR verification pool policy {field} must be positive")
    for field in ("gb_per_worker", "per_case_timeout_seconds"):
        value = policy.get(field)
        if not isinstance(value, (int, float)) or isinstance(value, bool) or value <= 0:
            raise ValueError(f"IR verification pool policy {field} must be positive")
    return payload


def _worker_lifetime_peak_rss_bytes() -> int:
    """Return the worker lifetime high-water RSS, including earlier cases."""
    if sys.platform == "win32":

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
                ("PrivateUsage", ctypes.c_size_t),
            ]

        counters = ProcessMemoryCounters()
        counters.cb = ctypes.sizeof(counters)
        process = ctypes.windll.kernel32.GetCurrentProcess()
        get_memory_info = ctypes.windll.kernel32.K32GetProcessMemoryInfo
        get_memory_info.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ProcessMemoryCounters),
            ctypes.c_ulong,
        ]
        get_memory_info.restype = ctypes.c_int
        ok = get_memory_info(
            process,
            ctypes.byref(counters),
            counters.cb,
        )
        return int(counters.PeakWorkingSetSize) if ok else 0
    try:
        import resource

        rss = int(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)
        return rss if sys.platform == "darwin" else rss * 1024
    except (ImportError, OSError, ValueError):
        return 0


def _format_verifier_errors(errors: list[object]) -> str:
    if not errors:
        return ""
    return "\n".join([f"ERRORS ({len(errors)}):", *(str(error) for error in errors)])


def _compile_diagnostic(exc: BaseException) -> dict[str, object]:
    if isinstance(exc, CompatibilityError):
        issue = exc.issue
        return {
            "kind": "compatibility",
            "code": issue.diagnostic_code,
            "feature": issue.feature,
            "feature_category": issue.feature_category,
            "tier": issue.tier,
            "impact": issue.impact,
            "location": issue.location,
            "alternative": issue.alternative,
        }
    if isinstance(exc, SyntaxError):
        return {
            "kind": "syntax",
            "code": None,
            "feature": exc.msg,
            "feature_category": "syntax",
            "tier": None,
            "impact": "high",
            "filename": exc.filename,
            "line": exc.lineno,
            "offset": exc.offset,
            "end_line": exc.end_lineno,
            "end_offset": exc.end_offset,
            "text": exc.text,
        }
    return {
        "kind": "exception",
        "code": None,
        "feature": type(exc).__name__,
        "feature_category": type(exc).__name__,
        "tier": None,
        "impact": "high",
    }


def _verify_source_worker(source: str) -> dict[str, object]:
    """Compile one source inside a supervised long-lived compiler worker."""
    from molt.frontend import compile_to_tir

    started = time.perf_counter()
    cpu_started = time.process_time()
    path = Path(source)
    compile_started = time.perf_counter()
    try:
        tir = compile_to_tir(path.read_text(encoding="utf-8"))
    except BaseException as exc:
        diagnostic = _compile_diagnostic(exc)
        expected_rejection = diagnostic["kind"] in {"compatibility", "syntax"}
        if diagnostic["kind"] == "compatibility":
            stderr = ""
        elif diagnostic["kind"] == "syntax":
            stderr = f"SyntaxError: {diagnostic['feature']}"
        else:
            stderr = traceback.format_exc()
        return {
            "source": source,
            "status": "unsupported" if expected_rejection else "error",
            "error": (
                "compile rejected as unsupported"
                if expected_rejection
                else "compile failed"
            ),
            "compile": {
                "status": "unsupported" if expected_rejection else "error",
                "returncode": 2 if expected_rejection else 1,
                "stderr": stderr,
                "diagnostic": diagnostic,
                "duration_seconds": time.perf_counter() - compile_started,
            },
            "duration_seconds": time.perf_counter() - started,
            "worker_cpu_seconds": time.process_time() - cpu_started,
            "worker_lifetime_peak_rss_bytes_after_case": (
                _worker_lifetime_peak_rss_bytes()
            ),
        }
    compile_observable = {
        "status": "pass",
        "returncode": 0,
        "stderr": "",
        "duration_seconds": time.perf_counter() - compile_started,
    }
    return {
        "source": source,
        "status": "compiled",
        "compile": compile_observable,
        "tir": tir,
        "duration_seconds": time.perf_counter() - started,
        "compiler_worker_cpu_seconds": time.process_time() - cpu_started,
        "worker_lifetime_peak_rss_bytes_after_case": (
            _worker_lifetime_peak_rss_bytes()
        ),
    }


def _finalize_compiled_result(
    result: dict[str, object],
    request_id: int,
    *,
    per_case_timeout: float,
) -> dict[str, object]:
    if result.get("status") != "compiled":
        return result
    tir = result.pop("tir", None)
    if not isinstance(tir, dict):
        result.update(
            {
                "status": "error",
                "error": "compiler worker returned no TIR",
                "stderr": "",
            }
        )
        return result
    verification_started = time.perf_counter()
    verification_cpu_started = time.process_time()
    remaining_seconds = per_case_timeout - float(result["duration_seconds"])
    if remaining_seconds <= 0:
        result.update(
            {
                "status": "error",
                "error": "verification deadline exhausted by compilation",
                "stderr": "",
            }
        )
        return result
    try:
        verification = verify_tir(
            tir,
            request_id=request_id,
            timeout_seconds=remaining_seconds,
        )
    except BaseException:
        result.update(
            {
                "status": "error",
                "error": "verification failed",
                "stderr": traceback.format_exc(),
            }
        )
        return result
    parent_verification_cpu = time.process_time() - verification_cpu_started
    verifier_cpu_seconds = (
        parent_verification_cpu
        if verification.verifier_pid is None
        else verification.verifier_cpu_seconds
    )
    result["duration_seconds"] = float(result["duration_seconds"]) + (
        time.perf_counter() - verification_started
    )
    result.update(
        {
            "status": "pass" if verification.ok else "fail",
            "verifier_request_id": request_id,
            "verifier_cpu_seconds": verifier_cpu_seconds,
            "worker_cpu_seconds": (
                float(result.get("compiler_worker_cpu_seconds", 0.0))
                + verifier_cpu_seconds
            ),
            "verifier_pid": verification.verifier_pid,
            "verifier_lifetime_peak_rss_bytes": (
                verification.verifier_lifetime_peak_rss_bytes
            ),
        }
    )
    if not verification.ok:
        result.update(
            {
                "returncode": 1,
                "detail": _format_verifier_errors(verification.errors),
            }
        )
    return result


def _supervised_worker(
    worker_id: int,
    connection: Connection,
    max_cases: int,
) -> None:
    """Serve a bounded number of cases, then retire for deterministic recycling."""
    startup_started = time.perf_counter()
    from molt.frontend import compile_to_tir  # noqa: F401

    startup_duration = time.perf_counter() - startup_started
    completed = 0
    pid = os.getpid()
    try:
        connection.send(
            (
                "ready",
                worker_id,
                pid,
                {
                    "startup_duration_seconds": startup_duration,
                    "startup_lifetime_peak_rss_bytes": (
                        _worker_lifetime_peak_rss_bytes()
                    ),
                },
            )
        )
        while True:
            task = connection.recv()
            if task is None:
                connection.send(("stopped", worker_id, pid, completed))
                return
            index, source = task
            result = _verify_source_worker(source)
            completed += 1
            connection.send(("result", worker_id, pid, index, result))
            if completed >= max_cases:
                connection.send(("retired", worker_id, pid, completed))
                return
            connection.send(("ready", worker_id, pid, None))
    finally:
        connection.close()


def _run_worker_pool(
    files: list[Path],
    *,
    worker_count: int,
    per_case_timeout: float,
    max_cases_per_worker: int,
    fail_fast: bool,
    resource_policy: dict[str, object],
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Own one verifier for the complete compiler-pool lifetime."""
    verifier_binary()
    try:
        return _run_worker_pool_owned(
            files,
            worker_count=worker_count,
            per_case_timeout=per_case_timeout,
            max_cases_per_worker=max_cases_per_worker,
            fail_fast=fail_fast,
            resource_policy=resource_policy,
        )
    finally:
        close_process_local_verifier()


def _run_worker_pool_owned(
    files: list[Path],
    *,
    worker_count: int,
    per_case_timeout: float,
    max_cases_per_worker: int,
    fail_fast: bool,
    resource_policy: dict[str, object],
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Supervise owned workers with stable ordering, deadlines, and recycling."""
    if fail_fast:
        worker_count = 1
    context = multiprocessing.get_context("spawn")
    started = time.perf_counter()
    results: list[dict[str, object] | None] = [None] * len(files)
    workers: dict[int, dict[str, object]] = {}
    launched_pids: list[int] = []
    lifecycle: list[dict[str, object]] = []
    supervisor_errors: list[str] = []
    next_worker_id = 0
    next_index = 0
    timeouts = 0
    stopped_early = False
    pending_verification: dict[int, tuple[int, dict[str, object]]] = {}
    max_pending_verifications = 0

    def launch_worker() -> None:
        nonlocal next_worker_id
        worker_id = next_worker_id
        next_worker_id += 1
        parent_connection, child_connection = context.Pipe(duplex=True)
        launched_at = time.perf_counter()
        process = context.Process(
            target=_supervised_worker,
            args=(worker_id, child_connection, max_cases_per_worker),
            name=f"molt-ir-compiler-{worker_id}",
        )
        process.start()
        child_connection.close()
        assert process.pid is not None
        launched_pids.append(process.pid)
        workers[worker_id] = {
            "process": process,
            "connection": parent_connection,
            "task_index": None,
            "task_started": None,
            "stop_sent": False,
            "launched_at": launched_at,
            "exit_observed_at": None,
            "pending_verifications": 0,
            "ready_waiting": False,
        }
        lifecycle.append(
            {
                "worker_id": worker_id,
                "pid": process.pid,
                "event": "launched",
                "at_seconds": launched_at - started,
            }
        )

    def stop_worker(
        worker_id: int,
        *,
        terminate: bool,
        reason: str,
        completed_cases: int | None = None,
    ) -> None:
        state = workers.pop(worker_id)
        process = state["process"]
        connection = state["connection"]
        actions: list[str] = []
        if terminate and process.is_alive():
            process.terminate()
            actions.append("terminate")
        process.join(timeout=5.0)
        if process.is_alive():
            process.kill()
            actions.append("kill_after_terminate_timeout")
            process.join(timeout=5.0)
        connection.close()
        lifecycle.append(
            {
                "worker_id": worker_id,
                "pid": process.pid,
                "event": "terminated",
                "reason": reason,
                "actions": actions,
                "exitcode": process.exitcode,
                "completed_cases": completed_cases,
                "at_seconds": time.perf_counter() - started,
            }
        )

    def replace_if_work_remains() -> None:
        if stopped_early:
            return
        unassigned = len(files) - next_index
        available = sum(
            state["task_index"] is None and not state["stop_sent"]
            for state in workers.values()
        )
        needed = min(worker_count - len(workers), max(0, unassigned - available))
        for _ in range(needed):
            launch_worker()

    def protocol_loss(worker_id: int, *, reason: str) -> None:
        state = workers.get(worker_id)
        if state is None:
            return
        process = state["process"]
        index = state["task_index"]
        task_started = state["task_started"]
        message = (
            f"worker {worker_id} pid {process.pid} lost terminal protocol "
            f"evidence: {reason}"
        )
        supervisor_errors.append(message)
        if index is not None and results[index] is None:
            results[index] = {
                "source": str(files[index]),
                "status": "error",
                "error": "worker protocol lost",
                "compile": {
                    "status": "error",
                    "returncode": process.exitcode,
                    "stderr": message,
                },
                "duration_seconds": (
                    None if task_started is None else time.monotonic() - task_started
                ),
                "worker_lifetime_peak_rss_bytes_after_case": None,
            }
        stop_worker(
            worker_id,
            terminate=process.is_alive(),
            reason="missing_terminal_protocol",
        )
        replace_if_work_remains()

    def handle_event(worker_id: int, event: object) -> None:
        nonlocal max_pending_verifications, next_index
        state = workers.get(worker_id)
        if state is None:
            return
        process = state["process"]
        event_kind, event_worker_id, event_pid, *payload = event
        if event_worker_id != worker_id or event_pid != process.pid:
            raise RuntimeError(
                "worker protocol identity mismatch: "
                f"owned=({worker_id}, {process.pid}) "
                f"observed=({event_worker_id}, {event_pid})"
            )
        if event_kind == "ready":
            startup = payload[0]
            if startup is not None:
                ready_at = time.perf_counter()
                lifecycle.append(
                    {
                        "worker_id": worker_id,
                        "pid": process.pid,
                        "event": "ready",
                        **startup,
                        "parent_launch_to_ready_seconds": (
                            ready_at - state["launched_at"]
                        ),
                        "at_seconds": ready_at - started,
                    }
                )
            state["ready_waiting"] = True
            dispatch_ready_worker(worker_id)
        elif event_kind == "result":
            index, result = payload
            if state["task_index"] != index:
                raise RuntimeError(
                    f"worker {worker_id} returned unexpected case {index}"
                )
            pending_verification[index] = (worker_id, result)
            state["pending_verifications"] += 1
            max_pending_verifications = max(
                max_pending_verifications,
                len(pending_verification),
            )
            state["task_index"] = None
            state["task_started"] = None
        elif event_kind in {"retired", "stopped"}:
            completed_cases = int(payload[0])
            if state["task_index"] is not None:
                raise RuntimeError(
                    f"worker {worker_id} terminated before reporting its active case"
                )
            stop_worker(
                worker_id,
                terminate=False,
                reason=f"protocol_{event_kind}",
                completed_cases=completed_cases,
            )
            replace_if_work_remains()
        else:
            raise RuntimeError(f"worker {worker_id} sent unknown event {event_kind!r}")

    def dispatch_ready_worker(worker_id: int) -> None:
        nonlocal next_index
        state = workers.get(worker_id)
        if state is None or not state["ready_waiting"]:
            return
        if int(state["pending_verifications"]) >= 2:
            return
        connection = state["connection"]
        try:
            if stopped_early or next_index >= len(files):
                connection.send(None)
                state["stop_sent"] = True
            else:
                index = next_index
                next_index += 1
                state["task_index"] = index
                state["task_started"] = time.monotonic()
                connection.send((index, str(files[index])))
            state["ready_waiting"] = False
        except (BrokenPipeError, EOFError, OSError) as exc:
            protocol_loss(worker_id, reason=f"supervisor send failed: {exc}")

    def finalize_one_pending() -> None:
        nonlocal stopped_early
        if not pending_verification:
            return
        index = min(pending_verification)
        worker_id, result = pending_verification.pop(index)
        result = _finalize_compiled_result(
            result,
            request_id=index,
            per_case_timeout=per_case_timeout,
        )
        results[index] = result
        state = workers.get(worker_id)
        if state is not None:
            state["pending_verifications"] -= 1
            dispatch_ready_worker(worker_id)
        if fail_fast and result["status"] != "pass":
            stopped_early = True

    for _ in range(worker_count):
        launch_worker()

    while workers or pending_verification:
        connections = [state["connection"] for state in workers.values()]
        ready_connections = (
            wait_connections(connections, timeout=0.05) if connections else []
        )
        for connection in ready_connections:
            worker_id = next(
                candidate
                for candidate, state in workers.items()
                if state["connection"] is connection
            )
            while worker_id in workers:
                try:
                    event = connection.recv()
                except EOFError:
                    protocol_loss(worker_id, reason="EOF before terminal frame")
                    break
                handle_event(worker_id, event)
                if worker_id not in workers or not connection.poll():
                    break

        finalize_one_pending()

        now = time.monotonic()
        for worker_id, state in list(workers.items()):
            process = state["process"]
            index = state["task_index"]
            task_started = state["task_started"]
            if (
                index is not None
                and task_started is not None
                and now - task_started > per_case_timeout
            ):
                results[index] = {
                    "source": str(files[index]),
                    "status": "error",
                    "error": "compile timed out",
                    "compile": {
                        "status": "timeout",
                        "returncode": None,
                        "stderr": "",
                    },
                    "duration_seconds": now - task_started,
                    "worker_lifetime_peak_rss_bytes_after_case": None,
                }
                timeouts += 1
                stop_worker(worker_id, terminate=True, reason="case_timeout")
                if not fail_fast and next_index < len(files):
                    launch_worker()
                elif fail_fast:
                    stopped_early = True
                continue
            connection = state["connection"]
            if not process.is_alive() and process.exitcode is not None:
                if state["exit_observed_at"] is None:
                    state["exit_observed_at"] = now
                elif now - state["exit_observed_at"] > 1.0:
                    protocol_loss(
                        worker_id,
                        reason=(
                            f"process exited {process.exitcode}; terminal frame "
                            "absent after 1.0s drain grace"
                        ),
                    )

        if stopped_early:
            for worker_id in list(workers):
                stop_worker(worker_id, terminate=True, reason="fail_fast")
            break

    completed = [result for result in results if result is not None]
    observed_peaks = [
        int(peak)
        for result in completed
        if (peak := result["worker_lifetime_peak_rss_bytes_after_case"]) is not None
    ]
    startup_latencies = [
        float(event["parent_launch_to_ready_seconds"])
        for event in lifecycle
        if event["event"] == "ready"
    ]
    duration_seconds = time.perf_counter() - started
    worker_cpu_seconds = sum(
        float(result.get("worker_cpu_seconds", 0.0)) for result in completed
    )
    compiler_worker_cpu_seconds = sum(
        float(result.get("compiler_worker_cpu_seconds", 0.0)) for result in completed
    )
    verifier_cpu_seconds = sum(
        float(result.get("verifier_cpu_seconds", 0.0)) for result in completed
    )
    verifier_pids = sorted(
        {
            int(pid)
            for result in completed
            if (pid := result.get("verifier_pid")) is not None
        }
    )
    verifier_peaks = [
        int(peak)
        for result in completed
        if (peak := result.get("verifier_lifetime_peak_rss_bytes")) is not None
    ]
    telemetry = {
        "authority": "supervised-compiler-pool-single-rust-verifier-v4",
        "workers": worker_count,
        "max_cases_per_worker": max_cases_per_worker,
        "process_launches": len(launched_pids),
        "worker_pids": launched_pids,
        "worker_timeouts": timeouts,
        "terminal_frame_drain_grace_seconds": 1.0,
        "supervisor_errors": supervisor_errors,
        "worker_lifecycle": lifecycle,
        "per_case_timeout_seconds": per_case_timeout,
        "verification_queue_bound": 2 * worker_count,
        "verification_queue_high_water": max_pending_verifications,
        "duration_seconds": duration_seconds,
        "worker_cpu_seconds": worker_cpu_seconds,
        "compiler_worker_cpu_seconds": compiler_worker_cpu_seconds,
        "verifier_cpu_seconds": verifier_cpu_seconds,
        "verifier_pids": verifier_pids,
        "verifier_lifetime_peak_rss_bytes": max(verifier_peaks, default=0),
        "average_worker_cpu_cores": (
            worker_cpu_seconds / duration_seconds if duration_seconds > 0 else 0.0
        ),
        "worker_cpu_utilization_fraction": (
            worker_cpu_seconds / (duration_seconds * worker_count)
            if duration_seconds > 0 and worker_count > 0
            else 0.0
        ),
        "worker_cpu_semantics": (
            "sum of per-case process CPU time divided by pool wall time and worker width"
        ),
        "worker_lifetime_peak_rss_bytes": max(observed_peaks, default=0),
        "worker_rss_semantics": "lifetime high-water after each case",
        "worker_startup_parent_launch_to_ready_seconds_max": max(
            startup_latencies,
            default=0.0,
        ),
        "worker_startup_parent_launch_to_ready_seconds_mean": (
            sum(startup_latencies) / len(startup_latencies)
            if startup_latencies
            else 0.0
        ),
        "worker_startup_semantics": (
            "parent process.start through child module import, frontend import, and ready frame"
        ),
        "resource_policy": resource_policy,
        "per_case_guard_artifacts": 0,
    }
    return completed, telemetry


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--dir",
        default="tests/differential/basic",
        help="Directory to scan for .py files (default: tests/differential/basic)",
    )
    parser.add_argument(
        "--glob",
        default="**/*.py",
        help="Glob pattern within --dir (default: **/*.py)",
    )
    parser.add_argument(
        "--fail-fast",
        action="store_true",
        help="Stop on first verification failure",
    )
    parser.add_argument(
        "--quiet", "-q", action="store_true", help="Only print failures"
    )
    parser.add_argument(
        "--examples",
        action="store_true",
        help="Also verify examples/*.py",
    )
    parser.add_argument(
        "--json-out",
        metavar="FILE",
        help="Write the complete fail-closed sweep result to FILE",
    )
    parser.add_argument(
        "--workers",
        type=int,
        default=None,
        help="Override the canonical resource-pressure worker count",
    )
    parser.add_argument(
        "--per-case-timeout",
        type=float,
        default=None,
        help="Override the calibrated per-source deadline",
    )
    parser.add_argument(
        "--max-cases-per-worker",
        type=int,
        default=None,
        help="Override the calibrated worker recycle threshold",
    )
    args = parser.parse_args()

    base = Path(args.dir)
    if not base.exists():
        print(f"ERROR: Directory not found: {base}", file=sys.stderr)
        return 2

    files = sorted(base.glob(args.glob))
    if args.examples:
        examples = Path("examples")
        if examples.exists():
            files.extend(sorted(examples.glob("*.py")))

    if not files:
        print(f"No .py files found in {base} with pattern {args.glob}")
        return 2
    if args.workers is not None and args.workers < 1:
        parser.error("--workers must be at least 1")
    if args.per_case_timeout is not None and args.per_case_timeout <= 0:
        parser.error("--per-case-timeout must be positive")
    if args.max_cases_per_worker is not None and args.max_cases_per_worker < 1:
        parser.error("--max-cases-per-worker must be at least 1")

    selected = len(files)
    pool_policy = _load_pool_policy()
    calibrated = pool_policy["policy"]
    pressure_plan = plan_resource_pressure(
        prefix="MOLT_IR_VERIFY",
        max_compile_slots=int(calibrated["max_workers"]),
        compile_gb_per_slot=float(calibrated["gb_per_worker"]),
    )
    planned_workers = pressure_plan.compile_max_slots
    worker_count = args.workers if args.workers is not None else planned_workers
    per_case_timeout = (
        args.per_case_timeout
        if args.per_case_timeout is not None
        else float(calibrated["per_case_timeout_seconds"])
    )
    max_cases_per_worker = (
        args.max_cases_per_worker
        if args.max_cases_per_worker is not None
        else int(calibrated["max_cases_per_worker"])
    )
    resource_policy = pressure_plan.to_json_dict()
    resource_policy["ir_pool_policy"] = pool_policy
    resource_policy["selected_workers"] = min(worker_count, selected)
    resource_policy["worker_override"] = args.workers
    results, pool_telemetry = _run_worker_pool(
        files,
        worker_count=min(worker_count, selected),
        per_case_timeout=per_case_timeout,
        max_cases_per_worker=max_cases_per_worker,
        fail_fast=args.fail_fast,
        resource_policy=resource_policy,
    )
    attempted = len(results)
    passed = sum(result["status"] == "pass" for result in results)
    failed = sum(result["status"] == "fail" for result in results)
    unsupported = sum(result["status"] == "unsupported" for result in results)
    errors = sum(result["status"] == "error" for result in results)
    supervisor_errors = list(pool_telemetry["supervisor_errors"])
    failure_details = [
        (str(result["source"]), str(result.get("detail", "")))
        for result in results
        if result["status"] == "fail"
    ]

    for result in results:
        source = result["source"]
        status = result["status"]
        if status == "pass":
            if not args.quiet:
                print(f"  PASS {source}")
        elif status == "error":
            if not args.quiet:
                print(f"  ERROR {source} ({result.get('error', 'compile failed')})")
        elif status == "unsupported":
            if not args.quiet:
                compile_result = result.get("compile")
                diagnostic = (
                    compile_result.get("diagnostic")
                    if isinstance(compile_result, dict)
                    else None
                )
                code = diagnostic.get("code") if isinstance(diagnostic, dict) else None
                category = (
                    diagnostic.get("feature_category")
                    if isinstance(diagnostic, dict)
                    else None
                )
                label = code or category or "typed rejection"
                print(f"  UNSUPPORTED {source} ({label})")
        else:
            detail = str(result.get("detail", ""))
            print(f"  FAIL {source}")
            for line in detail.splitlines()[:5]:
                print(f"       {line}")

    print(
        f"\nIR verification suite: {selected} selected | {attempted} attempted | "
        f"{passed} pass | {failed} fail | {unsupported} unsupported | {errors} error"
    )
    if failure_details:
        print("\nFailed files:")
        for path, detail in failure_details:
            print(f"  {path}")

    if args.json_out:
        complete_success = (
            passed + unsupported > 0
            and attempted == selected
            and selected - attempted == 0
            and failed == 0
            and errors == 0
            and not supervisor_errors
        )
        computed_status = "success" if complete_success else "failure"
        out = {
            "schema": "molt.ir-verification-sweep.v1",
            "status": computed_status,
            "selected": selected,
            "attempted": attempted,
            "unexecuted": selected - attempted,
            "executed": passed + failed,
            "passed": passed,
            "failed": failed,
            "unsupported": unsupported,
            "errors": errors,
            "supervisor_errors": supervisor_errors,
            "pool": pool_telemetry,
            "results": results,
        }
        output_path = Path(args.json_out)
        atomic_write_json(output_path, out, indent=2)

    return fail_closed_proof_exit_code(
        executed=passed + failed + unsupported,
        failed=failed,
        errors=(errors + len(supervisor_errors) + (selected - attempted)),
    )


if __name__ == "__main__":
    sys.exit(main())
