"""Long-lived transport client for the canonical Rust SimpleIR verifier."""

from __future__ import annotations

import atexit
import ctypes
import json
import math
import os
import queue
import subprocess
import sys
import threading
from collections.abc import Sequence
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from molt.dx import development_artifact_env  # noqa: E402
from tools.command_execution import CommandExecutor  # noqa: E402

_BINARY_NAME = "molt-ir-verify.exe" if os.name == "nt" else "molt-ir-verify"
_BUILD_LOCK = threading.Lock()
_BINARY_READY: set[Path] = set()
_PROCESS_LOCAL_LOCK = threading.Lock()
_COMMANDS = CommandExecutor.for_file(__file__)
_DEFAULT_REQUEST_TIMEOUT_SECONDS = 60.0


def _request_timeout_seconds(env: dict[str, str]) -> float:
    raw = env.get("MOLT_IR_VERIFIER_REQUEST_TIMEOUT_SEC", "").strip()
    if not raw:
        return _DEFAULT_REQUEST_TIMEOUT_SECONDS
    try:
        timeout = float(raw)
    except ValueError as exc:
        raise ValueError("IR verifier request timeout must be numeric") from exc
    if not math.isfinite(timeout) or timeout <= 0:
        raise ValueError("IR verifier request timeout must be finite and positive")
    return timeout


def _process_metrics(pid: int) -> tuple[float, int]:
    if os.name == "nt":

        class FileTime(ctypes.Structure):
            _fields_ = [("low", ctypes.c_ulong), ("high", ctypes.c_ulong)]

        class MemoryCounters(ctypes.Structure):
            _fields_ = [
                ("cb", ctypes.c_ulong),
                ("page_fault_count", ctypes.c_ulong),
                ("peak_working_set_size", ctypes.c_size_t),
                ("working_set_size", ctypes.c_size_t),
                ("quota_peak_paged_pool_usage", ctypes.c_size_t),
                ("quota_paged_pool_usage", ctypes.c_size_t),
                ("quota_peak_non_paged_pool_usage", ctypes.c_size_t),
                ("quota_non_paged_pool_usage", ctypes.c_size_t),
                ("pagefile_usage", ctypes.c_size_t),
                ("peak_pagefile_usage", ctypes.c_size_t),
                ("private_usage", ctypes.c_size_t),
            ]

        kernel32 = ctypes.windll.kernel32
        kernel32.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
        kernel32.OpenProcess.restype = ctypes.c_void_p
        kernel32.GetProcessTimes.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(FileTime),
            ctypes.POINTER(FileTime),
            ctypes.POINTER(FileTime),
            ctypes.POINTER(FileTime),
        ]
        kernel32.GetProcessTimes.restype = ctypes.c_int
        kernel32.K32GetProcessMemoryInfo.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(MemoryCounters),
            ctypes.c_ulong,
        ]
        kernel32.K32GetProcessMemoryInfo.restype = ctypes.c_int
        kernel32.CloseHandle.argtypes = [ctypes.c_void_p]
        handle = kernel32.OpenProcess(0x1410, False, pid)
        if not handle:
            return (0.0, 0)
        try:
            created = FileTime()
            exited = FileTime()
            kernel = FileTime()
            user = FileTime()
            cpu_seconds = 0.0
            if kernel32.GetProcessTimes(
                handle,
                ctypes.byref(created),
                ctypes.byref(exited),
                ctypes.byref(kernel),
                ctypes.byref(user),
            ):
                kernel_ticks = (kernel.high << 32) | kernel.low
                user_ticks = (user.high << 32) | user.low
                cpu_seconds = (kernel_ticks + user_ticks) / 10_000_000
            counters = MemoryCounters()
            counters.cb = ctypes.sizeof(counters)
            peak_rss = 0
            if ctypes.windll.kernel32.K32GetProcessMemoryInfo(
                handle,
                ctypes.byref(counters),
                counters.cb,
            ):
                peak_rss = int(counters.peak_working_set_size)
            return (cpu_seconds, peak_rss)
        finally:
            kernel32.CloseHandle(handle)
    if sys.platform == "darwin":

        class ProcTaskInfo(ctypes.Structure):
            _fields_ = [
                ("virtual_size", ctypes.c_uint64),
                ("resident_size", ctypes.c_uint64),
                ("total_user", ctypes.c_uint64),
                ("total_system", ctypes.c_uint64),
                ("threads_user", ctypes.c_uint64),
                ("threads_system", ctypes.c_uint64),
                ("policy", ctypes.c_int32),
                ("faults", ctypes.c_int32),
                ("pageins", ctypes.c_int32),
                ("cow_faults", ctypes.c_int32),
                ("messages_sent", ctypes.c_int32),
                ("messages_received", ctypes.c_int32),
                ("syscalls_mach", ctypes.c_int32),
                ("syscalls_unix", ctypes.c_int32),
                ("context_switches", ctypes.c_int32),
                ("thread_count", ctypes.c_int32),
                ("running_threads", ctypes.c_int32),
                ("priority", ctypes.c_int32),
            ]

        try:
            libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
            proc_pidinfo = libproc.proc_pidinfo
            proc_pidinfo.argtypes = [
                ctypes.c_int,
                ctypes.c_int,
                ctypes.c_uint64,
                ctypes.c_void_p,
                ctypes.c_int,
            ]
            proc_pidinfo.restype = ctypes.c_int
            task = ProcTaskInfo()
            written = proc_pidinfo(
                pid,
                4,  # PROC_PIDTASKINFO
                0,
                ctypes.byref(task),
                ctypes.sizeof(task),
            )
            if written == ctypes.sizeof(task):
                cpu_seconds = (task.total_user + task.total_system) / 1_000_000_000
                return cpu_seconds, int(task.resident_size)
        except (AttributeError, OSError):
            pass
        return (0.0, 0)
    stat_path = Path(f"/proc/{pid}/stat")
    status_path = Path(f"/proc/{pid}/status")
    try:
        stat_fields = stat_path.read_text(encoding="utf-8").split()
        clock_ticks = int(os.sysconf("SC_CLK_TCK"))
        cpu_seconds = (int(stat_fields[13]) + int(stat_fields[14])) / clock_ticks
        peak_rss = 0
        for line in status_path.read_text(encoding="utf-8").splitlines():
            if line.startswith("VmHWM:"):
                peak_rss = int(line.split()[1]) * 1024
                break
        return (cpu_seconds, peak_rss)
    except (FileNotFoundError, IndexError, OSError, ValueError):
        return (0.0, 0)


def _verifier_environment() -> dict[str, str]:
    return development_artifact_env(ROOT)


def verifier_binary(*, env: dict[str, str] | None = None) -> Path:
    env = _verifier_environment() if env is None else env
    target_root = Path(env["CARGO_TARGET_DIR"])
    binary = target_root / "debug" / _BINARY_NAME
    with _BUILD_LOCK:
        if binary not in _BINARY_READY:
            _COMMANDS.run(
                ["cargo", "build", "-p", "molt-ir", "--bin", "molt-ir-verify"],
                cwd=ROOT,
                env=env,
                check=True,
            )
            _BINARY_READY.add(binary)
    if not binary.is_file():
        raise FileNotFoundError(f"Rust IR verifier build did not produce {binary}")
    return binary


class RustIrVerifier:
    """One owned verifier process serving arbitrarily many IR documents."""

    def __init__(
        self,
        *,
        command: Sequence[str] | None = None,
        env: dict[str, str] | None = None,
        request_timeout_seconds: float | None = None,
    ) -> None:
        env = _verifier_environment() if env is None else dict(env)
        resolved_command = (
            [str(verifier_binary(env=env))]
            if command is None
            else [str(part) for part in command]
        )
        if not resolved_command:
            raise ValueError("IR verifier command must not be empty")
        self._request_timeout_seconds = (
            _request_timeout_seconds(env)
            if request_timeout_seconds is None
            else request_timeout_seconds
        )
        if (
            not math.isfinite(self._request_timeout_seconds)
            or self._request_timeout_seconds <= 0
        ):
            raise ValueError("IR verifier request timeout must be finite and positive")
        self._process = _COMMANDS.start_owned(
            resolved_command,
            cwd=ROOT,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            bufsize=1,
        )
        self._responses: queue.Queue[str | None] = queue.Queue()
        self._reader = threading.Thread(
            target=self._read_responses,
            name=f"molt-ir-verifier-reader-{self._process.pid}",
            daemon=True,
        )
        self._reader.start()
        self._lifetime_peak_rss_bytes = 0
        self._next_request_id = 0
        self._request_lock = threading.Lock()
        atexit.register(self.close)

    def _read_responses(self) -> None:
        stdout = self._process.stdout
        if stdout is None:
            self._responses.put(None)
            return
        try:
            for line in stdout:
                self._responses.put(line)
        finally:
            self._responses.put(None)

    @property
    def pid(self) -> int:
        return self._process.pid

    def verify(
        self,
        ir: dict[str, Any],
        *,
        request_id: int | None = None,
        timeout_seconds: float | None = None,
    ) -> dict[str, Any]:
        with self._request_lock:
            return self._verify_locked(
                ir,
                request_id=request_id,
                timeout_seconds=timeout_seconds,
            )

    def _verify_locked(
        self,
        ir: dict[str, Any],
        *,
        request_id: int | None,
        timeout_seconds: float | None,
    ) -> dict[str, Any]:
        if self._process.poll() is not None:
            raise RuntimeError(self._process_failure("before request"))
        if self._process.stdin is None or self._process.stdout is None:
            raise RuntimeError("Rust IR verifier pipes are unavailable")
        cpu_before, _peak_before = _process_metrics(self.pid)
        if request_id is None:
            request_id = self._next_request_id
            self._next_request_id += 1
        elif request_id < 0:
            raise ValueError("Rust IR verifier request id must be nonnegative")
        else:
            self._next_request_id = max(self._next_request_id, request_id + 1)
        request = json.dumps(
            {"id": request_id, "ir": ir},
            separators=(",", ":"),
            allow_nan=False,
        )
        self._process.stdin.write(request + "\n")
        self._process.stdin.flush()
        request_timeout = (
            self._request_timeout_seconds
            if timeout_seconds is None
            else timeout_seconds
        )
        if not math.isfinite(request_timeout) or request_timeout <= 0:
            raise ValueError("IR verifier request timeout must be finite and positive")
        try:
            response = self._responses.get(timeout=request_timeout)
        except queue.Empty as exc:
            self.close(graceful=False)
            raise TimeoutError(
                f"Rust IR verifier request {request_id} exceeded "
                f"{request_timeout:.3f}s"
            ) from exc
        if response is None:
            raise RuntimeError(self._process_failure("before response"))
        payload = json.loads(response)
        if not isinstance(payload, dict):
            raise RuntimeError("Rust IR verifier response is not an object")
        if payload.get("schema") != "molt.simple-ir-verification.v1":
            raise RuntimeError("Rust IR verifier response schema mismatch")
        transport_error = payload.get("transport_error")
        if transport_error is not None:
            raise ValueError(f"invalid SimpleIR transport: {transport_error}")
        if payload.get("id") != request_id:
            raise RuntimeError(
                "Rust IR verifier response id mismatch: "
                f"expected {request_id}, got {payload.get('id')!r}"
            )
        report = payload.get("report")
        if not isinstance(report, dict):
            raise RuntimeError("Rust IR verifier response has no report")
        cpu_after, rss_after = _process_metrics(self.pid)
        self._lifetime_peak_rss_bytes = max(
            self._lifetime_peak_rss_bytes,
            rss_after,
        )
        report["verifier_process"] = {
            "pid": self.pid,
            "cpu_seconds": max(0.0, cpu_after - cpu_before),
            "lifetime_peak_rss_bytes": self._lifetime_peak_rss_bytes,
        }
        return report

    def _process_failure(self, phase: str) -> str:
        stderr = ""
        if self._process.stderr is not None and self._process.poll() is not None:
            stderr = self._process.stderr.read().strip()
        return f"Rust IR verifier exited {self._process.returncode} {phase}" + (
            f": {stderr}" if stderr else ""
        )

    def close(self, *, graceful: bool = True) -> None:
        process = self._process
        if process.poll() is None:
            if graceful and process.stdin is not None:
                process.stdin.close()
            if not graceful:
                process.terminate()
            try:
                process.wait(timeout=5.0 if graceful else 1.0)
            except subprocess.TimeoutExpired:
                # Exact child custody: this object created this verifier process.
                if graceful:
                    process.terminate()
                    try:
                        process.wait(timeout=1.0)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=1.0)
                else:
                    process.kill()
                    process.wait(timeout=1.0)
        self._reader.join(timeout=1.0)


_PROCESS_LOCAL_VERIFIER: RustIrVerifier | None = None


def verify_ir(
    ir: dict[str, Any],
    *,
    request_id: int | None = None,
    timeout_seconds: float | None = None,
) -> dict[str, Any]:
    global _PROCESS_LOCAL_VERIFIER
    with _PROCESS_LOCAL_LOCK:
        if _PROCESS_LOCAL_VERIFIER is None:
            _PROCESS_LOCAL_VERIFIER = RustIrVerifier()
        verifier = _PROCESS_LOCAL_VERIFIER
        try:
            return verifier.verify(
                ir,
                request_id=request_id,
                timeout_seconds=timeout_seconds,
            )
        except (RuntimeError, TimeoutError):
            if _PROCESS_LOCAL_VERIFIER is verifier:
                _PROCESS_LOCAL_VERIFIER = None
            verifier.close()
            raise


def close_process_local_verifier() -> None:
    global _PROCESS_LOCAL_VERIFIER
    with _PROCESS_LOCAL_LOCK:
        verifier = _PROCESS_LOCAL_VERIFIER
        _PROCESS_LOCAL_VERIFIER = None
    if verifier is not None:
        verifier.close()


def process_local_verifier_pid() -> int | None:
    with _PROCESS_LOCAL_LOCK:
        verifier = _PROCESS_LOCAL_VERIFIER
    return None if verifier is None else verifier.pid
