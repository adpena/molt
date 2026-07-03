#!/usr/bin/env python3
"""Windows Job Object custody — kill a spawned build subtree the instant its
guard dies, closing the orphan-leak class at the SOURCE.

This is the structural complement to ``tools/orphan_reaper.py``. The reaper is a
periodic safety net (it sweeps dead-parent build tools every ~90s); this closes
the hole so orphans never form in the first place.

ROOT CAUSE (Windows): terminating a process does not kill its children, and the
memory guard's tree-reap only runs if the guard executes its own cleanup. If the
guard is OOM-killed, crashes, or its parent shell dies, the build subtree
(cargo -> rustc -> link -> tail) is orphaned and reserves GB.

FIX: create a job object with ``JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE``, spawn the
child SUSPENDED, ``AssignProcessToJobObject`` it, then resume it. Because the
child does no work until it is already in the job, every descendant it later
spawns inherits the job with zero race. The guard holds the sole (non-heritable)
job handle; when the guard dies for ANY reason the OS closes that handle and
reaps the entire subtree.

SAFETY / DEGRADATION: every operation is best-effort. If job creation, the
suspended spawn, assignment, or resume fails for any reason, callers fall back
to today's behavior (a normally-spawned child) and ``tools/orphan_reaper.py``
remains the backstop. The one hard guarantee: a child spawned SUSPENDED is
ALWAYS resumed, even when every job step fails, so a build can never hang
suspended. Non-Windows is a no-op.
"""
from __future__ import annotations

import subprocess
import sys
from typing import Any

_WINDOWS = sys.platform.startswith("win")

# Win32 constants.
CREATE_SUSPENDED = 0x00000004
_JOBOBJECT_EXTENDED_LIMIT_INFORMATION_CLASS = 9  # JobObjectExtendedLimitInformation
_JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000
_TH32CS_SNAPTHREAD = 0x00000004
_THREAD_SUSPEND_RESUME = 0x00000002
_INVALID_HANDLE_VALUE = -1


def suspended_creationflag() -> int:
    """CREATE_SUSPENDED on Windows, else 0. OR into existing creationflags."""
    return CREATE_SUSPENDED if _WINDOWS else 0


def _k32():
    import ctypes

    return ctypes.WinDLL("kernel32", use_last_error=True)


def create_kill_on_close_job() -> int | None:
    """Create an anonymous job with KILL_ON_JOB_CLOSE. Return handle or None."""
    if not _WINDOWS:
        return None
    import ctypes
    from ctypes import wintypes

    try:
        k32 = _k32()
        k32.CreateJobObjectW.restype = wintypes.HANDLE
        k32.CreateJobObjectW.argtypes = [wintypes.LPVOID, wintypes.LPCWSTR]
        job = k32.CreateJobObjectW(None, None)
        if not job:
            return None

        # JOBOBJECT_EXTENDED_LIMIT_INFORMATION: only LimitFlags matters here.
        class _IO_COUNTERS(ctypes.Structure):
            _fields_ = [(n, ctypes.c_ulonglong) for n in (
                "ReadOperationCount", "WriteOperationCount", "OtherOperationCount",
                "ReadTransferCount", "WriteTransferCount", "OtherTransferCount",
            )]

        class _BASIC(ctypes.Structure):
            _fields_ = [
                ("PerProcessUserTimeLimit", wintypes.LARGE_INTEGER),
                ("PerJobUserTimeLimit", wintypes.LARGE_INTEGER),
                ("LimitFlags", wintypes.DWORD),
                ("MinimumWorkingSetSize", ctypes.c_size_t),
                ("MaximumWorkingSetSize", ctypes.c_size_t),
                ("ActiveProcessLimit", wintypes.DWORD),
                ("Affinity", ctypes.POINTER(ctypes.c_ulong)),
                ("PriorityClass", wintypes.DWORD),
                ("SchedulingClass", wintypes.DWORD),
            ]

        class _EXTENDED(ctypes.Structure):
            _fields_ = [
                ("BasicLimitInformation", _BASIC),
                ("IoInfo", _IO_COUNTERS),
                ("ProcessMemoryLimit", ctypes.c_size_t),
                ("JobMemoryLimit", ctypes.c_size_t),
                ("PeakProcessMemoryUsed", ctypes.c_size_t),
                ("PeakJobMemoryUsed", ctypes.c_size_t),
            ]

        info = _EXTENDED()
        info.BasicLimitInformation.LimitFlags = _JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        k32.SetInformationJobObject.argtypes = [
            wintypes.HANDLE, ctypes.c_int, wintypes.LPVOID, wintypes.DWORD,
        ]
        ok = k32.SetInformationJobObject(
            job,
            _JOBOBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
            ctypes.byref(info),
            ctypes.sizeof(info),
        )
        if not ok:
            k32.CloseHandle(job)
            return None
        return int(job)
    except Exception:
        return None


def _resume_process_threads(pid: int) -> None:
    """ResumeThread every thread of ``pid`` (via a thread snapshot)."""
    import ctypes
    from ctypes import wintypes

    class THREADENTRY32(ctypes.Structure):
        _fields_ = [
            ("dwSize", wintypes.DWORD),
            ("cntUsage", wintypes.DWORD),
            ("th32ThreadID", wintypes.DWORD),
            ("th32OwnerProcessID", wintypes.DWORD),
            ("tpBasePri", ctypes.c_long),
            ("tpDeltaPri", ctypes.c_long),
            ("dwFlags", wintypes.DWORD),
        ]

    k32 = _k32()
    snap = k32.CreateToolhelp32Snapshot(_TH32CS_SNAPTHREAD, 0)
    if snap == _INVALID_HANDLE_VALUE:
        return
    try:
        entry = THREADENTRY32()
        entry.dwSize = ctypes.sizeof(THREADENTRY32)
        if not k32.Thread32First(snap, ctypes.byref(entry)):
            return
        while True:
            if entry.th32OwnerProcessID == pid:
                h = k32.OpenThread(_THREAD_SUSPEND_RESUME, False, entry.th32ThreadID)
                if h:
                    try:
                        k32.ResumeThread(h)
                    finally:
                        k32.CloseHandle(h)
            if not k32.Thread32Next(snap, ctypes.byref(entry)):
                break
    finally:
        k32.CloseHandle(snap)


def assign_and_resume(job: int | None, proc: subprocess.Popen[Any]) -> bool:
    """Assign ``proc`` to ``job`` then resume it. ALWAYS resumes.

    Returns True iff the process was successfully placed in the job. On any
    failure the process is still resumed (so a SUSPENDED child never hangs) and
    False is returned; the caller then relies on orphan_reaper as the backstop.
    """
    assigned = False
    if _WINDOWS and job:
        try:
            import ctypes
            from ctypes import wintypes

            handle = getattr(proc, "_handle", None)
            if handle is not None:
                k32 = _k32()
                k32.AssignProcessToJobObject.argtypes = [
                    wintypes.HANDLE, wintypes.HANDLE,
                ]
                assigned = bool(
                    k32.AssignProcessToJobObject(job, int(handle))
                )
        except Exception:
            assigned = False
    # Hard guarantee: a child spawned SUSPENDED is always resumed.
    if _WINDOWS:
        try:
            _resume_process_threads(proc.pid)
        except Exception:
            pass
    return assigned


def close_job(job: int | None) -> None:
    """Close the job handle. If it was the last handle, KILL_ON_JOB_CLOSE reaps
    any still-live members (a no-op once the build has exited)."""
    if not _WINDOWS or not job:
        return
    try:
        _k32().CloseHandle(int(job))
    except Exception:
        pass
