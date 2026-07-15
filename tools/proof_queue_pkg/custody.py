"""Queue-owned process identity, launch, memory-guard, and termination custody."""

from __future__ import annotations

import argparse
import ctypes
import os
import shlex
import subprocess
import sys
from pathlib import Path

from tools.process_spawn import (
    detached_process_group_kwargs,
    hidden_windows_process_group_kwargs,
)
from tools.proof_queue_pkg import state

MEMORY_GUARD_POLL_SEC_ENV = "MOLT_MEMORY_GUARD_POLL_SEC"

DEFAULT_PROOF_QUEUE_MEMORY_GUARD_POLL_SEC = "2.0"

PROOF_QUEUE_ACTIVE_POLL_SECONDS = 2.0

PROOF_QUEUE_DISPATCH_STALE_SECONDS = 120.0

PROOF_QUEUE_STALE_TERMINATE_GRACE_SECONDS = 5.0

PROOF_QUEUE_STALE_EXIT_CODE = 2

# Wall-clock ceiling on how long a row may remain 'running' before prune-stale
# reclaims it regardless of guard liveness. A detached runner that set
# status='running' then died before any terminal write leaves a row whose
# guard_pid Windows may recycle to an unrelated live process; the age ceiling
# guarantees such rows are reclaimed even when the recycled PID looks alive.
PROOF_QUEUE_RUNNING_AGE_CEILING_SECONDS = 6 * 60 * 60.0



def _terminate_queue_owned_guard_process(
    proc: subprocess.Popen[str],
    log: object,
    *,
    run_id: str,
) -> int | None:
    try:
        proc.terminate()
    except Exception as exc:  # pragma: no cover - host/process-shape specific.
        print(
            "proof_queue stale terminalization could not terminate "
            f"guard_pid={proc.pid} run_id={run_id}: {type(exc).__name__}: {exc}",
            file=log,
            flush=True,
        )
        return None
    try:
        return int(proc.wait(timeout=PROOF_QUEUE_STALE_TERMINATE_GRACE_SECONDS))
    except subprocess.TimeoutExpired:
        print(
            "proof_queue stale terminalization escalate kill "
            f"guard_pid={proc.pid} run_id={run_id}",
            file=log,
            flush=True,
        )
    try:
        proc.kill()
    except Exception as exc:  # pragma: no cover - host/process-shape specific.
        print(
            "proof_queue stale terminalization could not kill "
            f"guard_pid={proc.pid} run_id={run_id}: {type(exc).__name__}: {exc}",
            file=log,
            flush=True,
        )
        return None
    try:
        return int(proc.wait(timeout=PROOF_QUEUE_STALE_TERMINATE_GRACE_SECONDS))
    except subprocess.TimeoutExpired:  # pragma: no cover - only wedged OS child.
        print(
            "proof_queue stale terminalization guard still live after kill "
            f"guard_pid={proc.pid} run_id={run_id}",
            file=log,
            flush=True,
        )
        return None



def _memory_guard_command(
    *,
    command: list[str],
    summary_json: Path,
    timeout: float,
    poll_interval: str,
) -> list[str]:
    return [
        sys.executable,
        str(state.ROOT / "tools" / "memory_guard.py"),
        "--max-rss-gb",
        "12.0",
        "--max-total-rss-gb",
        "18.0",
        "--poll-interval",
        poll_interval,
        "--summary-json",
        str(summary_json),
        "--child-rlimit-gb",
        "12.0",
        "--timeout",
        str(timeout),
        "--",
        *command,
    ]



def _proof_queue_memory_guard_poll_sec(env_overrides: dict[str, str]) -> str:
    value = env_overrides.get(
        MEMORY_GUARD_POLL_SEC_ENV, DEFAULT_PROOF_QUEUE_MEMORY_GUARD_POLL_SEC
    ).strip()
    try:
        parsed = float(value)
    except ValueError as exc:
        raise ValueError(
            f"{MEMORY_GUARD_POLL_SEC_ENV} must be a positive finite number"
        ) from exc
    if parsed <= 0.0 or parsed == float("inf") or parsed != parsed:
        raise ValueError(
            f"{MEMORY_GUARD_POLL_SEC_ENV} must be a positive finite number"
        )
    return value



def _normalize_queue_process_environment() -> None:
    os.environ.setdefault("UV_LINK_MODE", "copy")



def _global_arg_pairs(args: argparse.Namespace) -> list[str]:
    pairs: list[str] = []
    for attr, option in (
        ("db", "--db"),
        ("logs_root", "--logs-root"),
        ("notebooks_root", "--notebooks-root"),
        ("repo_root", "--repo-root"),
    ):
        value = getattr(args, attr, None)
        if value:
            pairs.extend([option, str(value)])
    return pairs



def _launch_detached_runner(
    args: argparse.Namespace, *, run_id: str, timeout: float
) -> tuple[int, Path]:
    logs_root = state._logs_root(args)
    logs_root.mkdir(parents=True, exist_ok=True)
    runner_log = logs_root / f"{run_id}.runner.log"
    command = [
        sys.executable,
        str(state.ROOT / "tools" / "proof_queue.py"),
        *_global_arg_pairs(args),
        "run",
        "--run-id",
        run_id,
        "--limit",
        "1",
        "--timeout",
        str(timeout),
    ]
    popen_kwargs: dict[str, object] = {
        "cwd": state._repo_root(args),
        "stdin": subprocess.DEVNULL,
        "text": True,
    }
    popen_kwargs.update(
        detached_process_group_kwargs(
            windows=_queue_process_spawn_is_windows(),
            subprocess_module=subprocess,
        )
    )
    with runner_log.open("w", encoding="utf-8") as log:
        print(f"proof_queue detached runner for {run_id}", file=log, flush=True)
        print(f"command={shlex.join(command)}", file=log, flush=True)
        proc = subprocess.Popen(
            command,
            stdout=log,
            stderr=subprocess.STDOUT,
            **popen_kwargs,
        )
    return proc.pid, runner_log



def _queue_process_spawn_is_windows() -> bool:
    return os.name == "nt"



def _queued_command_process_kwargs() -> dict[str, object]:
    return hidden_windows_process_group_kwargs(
        windows=_queue_process_spawn_is_windows(),
        subprocess_module=subprocess,
    )



_PROCESS_QUERY_LIMITED_INFORMATION = 0x1000

_ERROR_ACCESS_DENIED = 5

_STILL_ACTIVE = 259



class _FILETIME(ctypes.Structure):
    _fields_ = [
        ("dwLowDateTime", ctypes.c_uint32),
        ("dwHighDateTime", ctypes.c_uint32),
    ]



def _windows_process_creation_ticks(pid: int) -> int | None:
    """Return the process creation time as 100ns ticks since 1601, or None.

    ``GetProcessTimes`` fills ``lpCreationTime`` with a FILETIME (a 64-bit
    count of 100-nanosecond units since 1601-01-01 UTC, per
    learn.microsoft.com GetProcessTimes). Combined with the PID this uniquely
    identifies a process across PID reuse: a recycled PID belongs to a process
    started later, so its creation time differs.
    """
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    handle = kernel32.OpenProcess(
        _PROCESS_QUERY_LIMITED_INFORMATION,
        False,
        int(pid),
    )
    if not handle:
        return None
    try:
        creation = _FILETIME()
        exit_time = _FILETIME()
        kernel_time = _FILETIME()
        user_time = _FILETIME()
        if not kernel32.GetProcessTimes(
            handle,
            ctypes.byref(creation),
            ctypes.byref(exit_time),
            ctypes.byref(kernel_time),
            ctypes.byref(user_time),
        ):
            return None
        return (creation.dwHighDateTime << 32) | creation.dwLowDateTime
    finally:
        kernel32.CloseHandle(handle)



def _process_creation_ticks(pid: int) -> int | None:
    """Best-effort process creation time (100ns ticks) for identity binding."""
    if pid <= 0:
        return None
    if os.name == "nt":
        try:
            return _windows_process_creation_ticks(pid)
        except OSError:
            return None
    # POSIX: /proc/<pid>/stat field 22 (starttime) is monotonic per boot and
    # differs across PID reuse. Fall back to None when unavailable so callers
    # degrade to the wall-clock running-age ceiling rather than a false match.
    try:
        with open(f"/proc/{pid}/stat", "rb") as handle:
            data = handle.read()
    except OSError:
        return None
    # comm may contain spaces/parens; split on the last ')' to skip it safely.
    rparen = data.rfind(b")")
    if rparen == -1:
        return None
    fields = data[rparen + 2 :].split()
    # After comm, field indices are shifted by 2 (pid, comm consumed); the
    # 22nd overall field (starttime) is index 19 of the post-comm split.
    if len(fields) <= 19:
        return None
    try:
        return int(fields[19])
    except ValueError:
        return None



def _process_identity(pid: int) -> str | None:
    """A PID+creation-time identity string, or None if it can't be determined.

    Used to detect Windows PID reuse: a guard row records this at launch, and
    prune-stale reclaims the row when the live PID's identity no longer matches
    (the original guard exited and the OS recycled its PID to a different
    process).
    """
    if pid is None or int(pid) <= 0:
        return None
    ticks = _process_creation_ticks(int(pid))
    if ticks is None:
        return None
    return f"{os.name}:{int(pid)}:{ticks}"



def _pid_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    if os.name == "nt":
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        handle = kernel32.OpenProcess(
            _PROCESS_QUERY_LIMITED_INFORMATION,
            False,
            int(pid),
        )
        if not handle:
            return ctypes.get_last_error() == _ERROR_ACCESS_DENIED
        try:
            exit_code = ctypes.c_ulong()
            if not kernel32.GetExitCodeProcess(handle, ctypes.byref(exit_code)):
                return False
            return exit_code.value == _STILL_ACTIVE
        finally:
            kernel32.CloseHandle(handle)
    try:
        os.kill(pid, 0)
    except OSError:
        return False
    return True



def _guard_process_live(pid: int | None, recorded_identity: str | None) -> bool:
    """True only when the guard PID is alive AND still the recorded process.

    ``_pid_alive`` is a bare liveness probe: on Windows it cannot distinguish a
    real exit code of 259 from STILL_ACTIVE, treats ERROR_ACCESS_DENIED as
    alive, and — critically — reports True for whatever unrelated process now
    owns a recycled PID. When an identity was recorded at launch we require the
    live process to still match it, so a recycled PID no longer looks like a
    live guard. When no identity was recorded (older row, or identity
    unavailable) we fall back to bare liveness and rely on the running-age
    ceiling to reclaim genuinely dead rows.
    """
    if pid is None:
        return False
    pid = int(pid)
    if not _pid_alive(pid):
        return False
    if not recorded_identity:
        return True
    current_identity = _process_identity(pid)
    if current_identity is None:
        # Can't re-derive identity (e.g. access denied) but the PID is alive;
        # treat as live so we don't falsely reclaim an active guard.
        return True
    return current_identity == recorded_identity
