#!/usr/bin/env python3
"""Fail-closed Windows Job Object custody for spawned process trees.

Windows does not recursively terminate descendants when a parent process dies.
This module closes that race at process creation: create a
``KILL_ON_JOB_CLOSE`` job, spawn the direct child suspended, assign it to the
job, and only then resume it.  Every later descendant inherits the job.

Custody failures are never downgraded to an unguarded child.  A child that
cannot be assigned is terminated while still suspended; a child that cannot be
resumed is terminated through the job.  Win32 failures are surfaced with their
error codes so callers can preserve precise incident evidence.
"""

from __future__ import annotations

import contextlib
import ctypes
from dataclasses import dataclass
import subprocess
import sys
import time
from ctypes import wintypes
from functools import lru_cache
from typing import Any

_WINDOWS = sys.platform.startswith("win")

CREATE_SUSPENDED = 0x00000004
_JOBOBJECT_BASIC_ACCOUNTING_INFORMATION_CLASS = 1
_JOBOBJECT_BASIC_PROCESS_ID_LIST_CLASS = 3
_JOBOBJECT_EXTENDED_LIMIT_INFORMATION_CLASS = 9
_JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000
_TH32CS_SNAPTHREAD = 0x00000004
_THREAD_SUSPEND_RESUME = 0x00000002
_PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
_PROCESS_VM_READ = 0x0010
_INVALID_HANDLE_VALUE = ctypes.c_void_p(-1).value
_ERROR_MORE_DATA = 234


class WinJobError(RuntimeError):
    """A Windows Job Object custody invariant could not be established."""


@dataclass(frozen=True, slots=True)
class WindowsJobAccounting:
    """Kernel-maintained lifetime accounting for one exact Job Object."""

    total_processes: int
    active_processes: int
    total_terminated_processes: int
    peak_job_commit_bytes: int


@dataclass(frozen=True, slots=True)
class WindowsSystemResources:
    """Process/handle/commit pressure captured without spawning a subprocess."""

    process_count: int | None
    thread_count: int | None
    system_handle_count: int | None
    guard_handle_count: int | None
    commit_total_bytes: int | None
    commit_limit_bytes: int | None
    commit_peak_bytes: int | None
    physical_total_bytes: int | None
    physical_available_bytes: int | None
    errors: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class WindowsJobCleanup:
    """Proof that an exact Job Object was empty before custody was released."""

    before: WindowsJobAccounting
    after: WindowsJobAccounting
    system_before: WindowsSystemResources
    system_after: WindowsSystemResources
    terminated_remaining_processes: bool
    elapsed_s: float

    @property
    def completed(self) -> bool:
        return self.after.active_processes == 0


class _IO_COUNTERS(ctypes.Structure):
    _fields_ = [
        (name, ctypes.c_ulonglong)
        for name in (
            "ReadOperationCount",
            "WriteOperationCount",
            "OtherOperationCount",
            "ReadTransferCount",
            "WriteTransferCount",
            "OtherTransferCount",
        )
    ]


class _BASIC_LIMIT_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("PerProcessUserTimeLimit", wintypes.LARGE_INTEGER),
        ("PerJobUserTimeLimit", wintypes.LARGE_INTEGER),
        ("LimitFlags", wintypes.DWORD),
        ("MinimumWorkingSetSize", ctypes.c_size_t),
        ("MaximumWorkingSetSize", ctypes.c_size_t),
        ("ActiveProcessLimit", wintypes.DWORD),
        ("Affinity", ctypes.c_size_t),
        ("PriorityClass", wintypes.DWORD),
        ("SchedulingClass", wintypes.DWORD),
    ]


class _EXTENDED_LIMIT_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("BasicLimitInformation", _BASIC_LIMIT_INFORMATION),
        ("IoInfo", _IO_COUNTERS),
        ("ProcessMemoryLimit", ctypes.c_size_t),
        ("JobMemoryLimit", ctypes.c_size_t),
        ("PeakProcessMemoryUsed", ctypes.c_size_t),
        ("PeakJobMemoryUsed", ctypes.c_size_t),
    ]


class _BASIC_ACCOUNTING_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("TotalUserTime", wintypes.LARGE_INTEGER),
        ("TotalKernelTime", wintypes.LARGE_INTEGER),
        ("ThisPeriodTotalUserTime", wintypes.LARGE_INTEGER),
        ("ThisPeriodTotalKernelTime", wintypes.LARGE_INTEGER),
        ("TotalPageFaultCount", wintypes.DWORD),
        ("TotalProcesses", wintypes.DWORD),
        ("ActiveProcesses", wintypes.DWORD),
        ("TotalTerminatedProcesses", wintypes.DWORD),
    ]


class _THREADENTRY32(ctypes.Structure):
    _fields_ = [
        ("dwSize", wintypes.DWORD),
        ("cntUsage", wintypes.DWORD),
        ("th32ThreadID", wintypes.DWORD),
        ("th32OwnerProcessID", wintypes.DWORD),
        ("tpBasePri", ctypes.c_long),
        ("tpDeltaPri", ctypes.c_long),
        ("dwFlags", wintypes.DWORD),
    ]


class _PROCESS_MEMORY_COUNTERS_EX(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("PageFaultCount", wintypes.DWORD),
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


class _PERFORMANCE_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("CommitTotal", ctypes.c_size_t),
        ("CommitLimit", ctypes.c_size_t),
        ("CommitPeak", ctypes.c_size_t),
        ("PhysicalTotal", ctypes.c_size_t),
        ("PhysicalAvailable", ctypes.c_size_t),
        ("SystemCache", ctypes.c_size_t),
        ("KernelTotal", ctypes.c_size_t),
        ("KernelPaged", ctypes.c_size_t),
        ("KernelNonpaged", ctypes.c_size_t),
        ("PageSize", ctypes.c_size_t),
        ("HandleCount", wintypes.DWORD),
        ("ProcessCount", wintypes.DWORD),
        ("ThreadCount", wintypes.DWORD),
    ]


def suspended_creationflag() -> int:
    """Return ``CREATE_SUSPENDED`` on Windows and zero elsewhere."""

    return CREATE_SUSPENDED if _WINDOWS else 0


@lru_cache(maxsize=1)
def _k32() -> Any:
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    k32.CreateJobObjectW.restype = wintypes.HANDLE
    k32.CreateJobObjectW.argtypes = [wintypes.LPVOID, wintypes.LPCWSTR]
    k32.SetInformationJobObject.restype = wintypes.BOOL
    k32.SetInformationJobObject.argtypes = [
        wintypes.HANDLE,
        ctypes.c_int,
        wintypes.LPVOID,
        wintypes.DWORD,
    ]
    k32.QueryInformationJobObject.restype = wintypes.BOOL
    k32.QueryInformationJobObject.argtypes = [
        wintypes.HANDLE,
        ctypes.c_int,
        wintypes.LPVOID,
        wintypes.DWORD,
        ctypes.POINTER(wintypes.DWORD),
    ]
    k32.AssignProcessToJobObject.restype = wintypes.BOOL
    k32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
    k32.TerminateJobObject.restype = wintypes.BOOL
    k32.TerminateJobObject.argtypes = [wintypes.HANDLE, wintypes.UINT]
    k32.TerminateProcess.restype = wintypes.BOOL
    k32.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
    k32.CloseHandle.restype = wintypes.BOOL
    k32.CloseHandle.argtypes = [wintypes.HANDLE]
    k32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    k32.CreateToolhelp32Snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
    k32.Thread32First.restype = wintypes.BOOL
    k32.Thread32First.argtypes = [wintypes.HANDLE, wintypes.LPVOID]
    k32.Thread32Next.restype = wintypes.BOOL
    k32.Thread32Next.argtypes = [wintypes.HANDLE, wintypes.LPVOID]
    k32.OpenThread.restype = wintypes.HANDLE
    k32.OpenThread.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    k32.ResumeThread.restype = wintypes.DWORD
    k32.ResumeThread.argtypes = [wintypes.HANDLE]
    k32.OpenProcess.restype = wintypes.HANDLE
    k32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    k32.GetCurrentProcess.restype = wintypes.HANDLE
    k32.GetCurrentProcess.argtypes = []
    k32.GetProcessHandleCount.restype = wintypes.BOOL
    k32.GetProcessHandleCount.argtypes = [
        wintypes.HANDLE,
        ctypes.POINTER(wintypes.DWORD),
    ]
    return k32


@lru_cache(maxsize=1)
def _psapi() -> Any:
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    psapi.GetProcessMemoryInfo.restype = wintypes.BOOL
    psapi.GetProcessMemoryInfo.argtypes = [
        wintypes.HANDLE,
        wintypes.LPVOID,
        wintypes.DWORD,
    ]
    psapi.GetPerformanceInfo.restype = wintypes.BOOL
    psapi.GetPerformanceInfo.argtypes = [
        ctypes.POINTER(_PERFORMANCE_INFORMATION),
        wintypes.DWORD,
    ]
    return psapi


def _win32_error(operation: str, *, code: int | None = None) -> WinJobError:
    resolved = ctypes.get_last_error() if code is None else code
    detail = ctypes.FormatError(resolved).strip() if resolved else "unknown error"
    return WinJobError(f"{operation} failed: winerror={resolved} detail={detail}")


def _close_handle(handle: int, *, operation: str) -> None:
    if not _k32().CloseHandle(wintypes.HANDLE(handle)):
        raise _win32_error(operation)


def create_kill_on_close_job() -> int | None:
    """Create the sole ``KILL_ON_JOB_CLOSE`` handle.

    Non-Windows callers receive ``None`` because Job Objects do not exist there.
    On Windows, inability to establish custody is an error rather than a request
    to continue with an unguarded process.
    """

    if not _WINDOWS:
        return None
    k32 = _k32()
    job = k32.CreateJobObjectW(None, None)
    if not job:
        raise _win32_error("CreateJobObjectW")
    job_handle = int(job)
    info = _EXTENDED_LIMIT_INFORMATION()
    info.BasicLimitInformation.LimitFlags = _JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    if not k32.SetInformationJobObject(
        wintypes.HANDLE(job_handle),
        _JOBOBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
        ctypes.byref(info),
        ctypes.sizeof(info),
    ):
        error = _win32_error("SetInformationJobObject")
        try:
            _close_handle(job_handle, operation="CloseHandle(after setup failure)")
        except WinJobError as close_error:
            error.add_note(str(close_error))
        raise error
    return job_handle


def _resume_process_threads(pid: int) -> None:
    """Resume every thread of ``pid`` or fail with exact Win32 evidence."""

    k32 = _k32()
    snap = k32.CreateToolhelp32Snapshot(_TH32CS_SNAPTHREAD, 0)
    if int(snap) == _INVALID_HANDLE_VALUE:
        raise _win32_error("CreateToolhelp32Snapshot")
    resumed = 0
    try:
        entry = _THREADENTRY32()
        entry.dwSize = ctypes.sizeof(_THREADENTRY32)
        if not k32.Thread32First(snap, ctypes.byref(entry)):
            raise _win32_error("Thread32First")
        while True:
            if entry.th32OwnerProcessID == pid:
                thread = k32.OpenThread(
                    _THREAD_SUSPEND_RESUME,
                    False,
                    entry.th32ThreadID,
                )
                if not thread:
                    raise _win32_error(f"OpenThread(tid={entry.th32ThreadID})")
                try:
                    previous_count = k32.ResumeThread(thread)
                    if previous_count == 0xFFFFFFFF:
                        raise _win32_error(f"ResumeThread(tid={entry.th32ThreadID})")
                    resumed += 1
                finally:
                    _close_handle(int(thread), operation="CloseHandle(thread)")
            if not k32.Thread32Next(snap, ctypes.byref(entry)):
                error = ctypes.get_last_error()
                if error not in {0, 18}:  # ERROR_NO_MORE_FILES
                    raise _win32_error("Thread32Next", code=error)
                break
    finally:
        _close_handle(int(snap), operation="CloseHandle(thread snapshot)")
    if resumed == 0:
        raise WinJobError(f"no resumable thread found for suspended pid={pid}")


def _resume_process(handle: int, pid: int) -> None:
    """Resume a suspended process, retaining a documented thread fallback."""

    primary_error: BaseException | None = None
    try:
        ntdll = ctypes.WinDLL("ntdll", use_last_error=True)
        ntdll.NtResumeProcess.argtypes = [wintypes.HANDLE]
        ntdll.NtResumeProcess.restype = ctypes.c_long
        status = int(ntdll.NtResumeProcess(wintypes.HANDLE(handle)))
        if status == 0:
            return
        primary_error = WinJobError(
            f"NtResumeProcess failed: ntstatus=0x{status & 0xFFFFFFFF:08x}"
        )
    except BaseException as exc:
        primary_error = exc
    try:
        _resume_process_threads(pid)
    except BaseException as fallback_error:
        error = WinJobError(f"could not resume suspended pid={pid}")
        if primary_error is not None:
            error.add_note(f"NtResumeProcess: {primary_error}")
        error.add_note(f"thread fallback: {fallback_error}")
        raise error from fallback_error


def terminate_job(job: int | None, *, exit_code: int = 1) -> None:
    """Terminate every process in ``job`` or surface the Win32 failure."""

    if not _WINDOWS:
        return
    if not job:
        raise WinJobError("Windows job handle is required")
    if not _k32().TerminateJobObject(wintypes.HANDLE(job), exit_code):
        raise _win32_error("TerminateJobObject")


def _basic_accounting_information(job: int | None) -> _BASIC_ACCOUNTING_INFORMATION:
    if not _WINDOWS:
        return _BASIC_ACCOUNTING_INFORMATION()
    if not job:
        raise WinJobError("Windows job handle is required")
    info = _BASIC_ACCOUNTING_INFORMATION()
    returned = wintypes.DWORD()
    if not _k32().QueryInformationJobObject(
        wintypes.HANDLE(job),
        _JOBOBJECT_BASIC_ACCOUNTING_INFORMATION_CLASS,
        ctypes.byref(info),
        ctypes.sizeof(info),
        ctypes.byref(returned),
    ):
        raise _win32_error("QueryInformationJobObject")
    return info


def active_process_count(job: int | None) -> int:
    """Return the exact number of live processes still owned by ``job``."""

    return int(_basic_accounting_information(job).ActiveProcesses)


def process_ids(job: int | None) -> tuple[int, ...]:
    """Snapshot every process id in ``job`` without a fixed-size ceiling."""

    if not _WINDOWS:
        return ()
    if not job:
        raise WinJobError("Windows job handle is required")
    capacity = 16
    while True:
        list_type = type(
            "_PROCESS_ID_LIST",
            (ctypes.Structure,),
            {
                "_fields_": [
                    ("NumberOfAssignedProcesses", wintypes.DWORD),
                    ("NumberOfProcessIdsInList", wintypes.DWORD),
                    ("ProcessIdList", ctypes.c_size_t * capacity),
                ]
            },
        )
        info = list_type()
        returned = wintypes.DWORD()
        if _k32().QueryInformationJobObject(
            wintypes.HANDLE(job),
            _JOBOBJECT_BASIC_PROCESS_ID_LIST_CLASS,
            ctypes.byref(info),
            ctypes.sizeof(info),
            ctypes.byref(returned),
        ):
            count = int(info.NumberOfProcessIdsInList)
            return tuple(int(info.ProcessIdList[index]) for index in range(count))
        error = ctypes.get_last_error()
        if error != _ERROR_MORE_DATA:
            raise _win32_error("QueryInformationJobObject(process ids)", code=error)
        capacity = max(capacity * 2, int(info.NumberOfAssignedProcesses), 1)


def current_working_set_bytes(job: int | None) -> int:
    """Sum the current working sets of a snapshot of live job members."""

    if not _WINDOWS:
        return 0
    k32 = _k32()
    total = 0
    for pid in process_ids(job):
        process = k32.OpenProcess(
            _PROCESS_QUERY_LIMITED_INFORMATION | _PROCESS_VM_READ,
            False,
            pid,
        )
        if not process:
            # The member may have exited after the job snapshot.  Re-read the
            # job rather than turning a normal exit race into telemetry failure.
            if pid not in process_ids(job):
                continue
            raise _win32_error(f"OpenProcess(pid={pid})")
        try:
            counters = _PROCESS_MEMORY_COUNTERS_EX()
            counters.cb = ctypes.sizeof(counters)
            if not _psapi().GetProcessMemoryInfo(
                process,
                ctypes.byref(counters),
                counters.cb,
            ):
                raise _win32_error(f"GetProcessMemoryInfo(pid={pid})")
            total += int(counters.WorkingSetSize)
        finally:
            _close_handle(int(process), operation="CloseHandle(process telemetry)")
    return total


def peak_job_memory_bytes(job: int | None) -> int:
    """Return the kernel-maintained peak aggregate memory for ``job``."""

    if not _WINDOWS:
        return 0
    if not job:
        raise WinJobError("Windows job handle is required")
    info = _EXTENDED_LIMIT_INFORMATION()
    returned = wintypes.DWORD()
    if not _k32().QueryInformationJobObject(
        wintypes.HANDLE(job),
        _JOBOBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
        ctypes.byref(info),
        ctypes.sizeof(info),
        ctypes.byref(returned),
    ):
        raise _win32_error("QueryInformationJobObject(extended limits)")
    return int(info.PeakJobMemoryUsed)


def job_accounting(job: int | None) -> WindowsJobAccounting:
    """Return lifetime process and peak-commit accounting for ``job``."""

    info = _basic_accounting_information(job)
    return WindowsJobAccounting(
        total_processes=int(info.TotalProcesses),
        active_processes=int(info.ActiveProcesses),
        total_terminated_processes=int(info.TotalTerminatedProcesses),
        peak_job_commit_bytes=peak_job_memory_bytes(job),
    )


def system_resources() -> WindowsSystemResources:
    """Capture Windows system pressure without allocating child processes.

    Resource telemetry must never weaken process custody.  Individual Win32
    query failures are retained in ``errors`` while the independent fields
    remain available.
    """

    if not _WINDOWS:
        return WindowsSystemResources(
            process_count=None,
            thread_count=None,
            system_handle_count=None,
            guard_handle_count=None,
            commit_total_bytes=None,
            commit_limit_bytes=None,
            commit_peak_bytes=None,
            physical_total_bytes=None,
            physical_available_bytes=None,
        )
    errors: list[str] = []
    performance: _PERFORMANCE_INFORMATION | None = _PERFORMANCE_INFORMATION()
    performance.cb = ctypes.sizeof(_PERFORMANCE_INFORMATION)
    if not _psapi().GetPerformanceInfo(
        ctypes.byref(performance),
        performance.cb,
    ):
        errors.append(str(_win32_error("GetPerformanceInfo")))
        performance = None
    handle_count = wintypes.DWORD()
    if not _k32().GetProcessHandleCount(
        _k32().GetCurrentProcess(),
        ctypes.byref(handle_count),
    ):
        errors.append(str(_win32_error("GetProcessHandleCount")))
        guard_handle_count: int | None = None
    else:
        guard_handle_count = int(handle_count.value)
    page_size = None if performance is None else int(performance.PageSize)

    def _bytes(field: str) -> int | None:
        if performance is None or page_size is None:
            return None
        return int(getattr(performance, field)) * page_size

    return WindowsSystemResources(
        process_count=(None if performance is None else int(performance.ProcessCount)),
        thread_count=(None if performance is None else int(performance.ThreadCount)),
        system_handle_count=(
            None if performance is None else int(performance.HandleCount)
        ),
        guard_handle_count=guard_handle_count,
        commit_total_bytes=_bytes("CommitTotal"),
        commit_limit_bytes=_bytes("CommitLimit"),
        commit_peak_bytes=_bytes("CommitPeak"),
        physical_total_bytes=_bytes("PhysicalTotal"),
        physical_available_bytes=_bytes("PhysicalAvailable"),
        errors=tuple(errors),
    )


def wait_until_empty(
    job: int | None,
    *,
    timeout: float,
    poll_interval: float = 0.01,
) -> None:
    """Wait until ``job`` owns no live process, failing on timeout."""

    deadline = time.monotonic() + timeout
    while True:
        active = active_process_count(job)
        if active == 0:
            return
        if time.monotonic() >= deadline:
            raise TimeoutError(
                f"Windows job still owns {active} process(es) after {timeout:.3f}s"
            )
        time.sleep(min(poll_interval, max(0.0, deadline - time.monotonic())))


def complete_job_custody(
    job: int | None,
    *,
    timeout: float,
) -> WindowsJobCleanup | None:
    """Make exact Job emptiness the completion boundary for guarded work.

    A direct child can exit while grandchildren remain alive.  Closing a
    ``KILL_ON_JOB_CLOSE`` handle only signals those descendants; it does not
    wait for their DLLs, handles, and commit charge to be released.  This
    primitive retains the sole exact-Job authority, terminates any remaining
    members, and waits for kernel accounting to reach zero before the caller
    may publish success or start the next process.
    """

    if not _WINDOWS:
        return None
    if not job:
        raise WinJobError("Windows job handle is required")
    started = time.monotonic()
    system_before = system_resources()
    before = job_accounting(job)
    terminated_remaining = before.active_processes > 0
    if terminated_remaining:
        terminate_job(job)
    wait_until_empty(job, timeout=timeout)
    after = job_accounting(job)
    if after.active_processes != 0:
        raise WinJobError(
            "Windows job completion returned with "
            f"{after.active_processes} active process(es)"
        )
    return WindowsJobCleanup(
        before=before,
        after=after,
        system_before=system_before,
        system_after=system_resources(),
        terminated_remaining_processes=terminated_remaining,
        elapsed_s=max(0.0, time.monotonic() - started),
    )


def assign_and_resume(job: int | None, proc: subprocess.Popen[Any]) -> None:
    """Assign a suspended child before resuming it; fail closed on either step."""

    if not _WINDOWS:
        return
    if not job:
        raise WinJobError("Windows job handle is required")
    handle_value = getattr(proc, "_handle", None)
    if handle_value is None:
        raise WinJobError("Popen process handle is unavailable")
    handle = int(handle_value)
    k32 = _k32()
    if not k32.AssignProcessToJobObject(
        wintypes.HANDLE(job),
        wintypes.HANDLE(handle),
    ):
        assignment_error = _win32_error("AssignProcessToJobObject")
        if not k32.TerminateProcess(wintypes.HANDLE(handle), 1):
            assignment_error.add_note(str(_win32_error("TerminateProcess")))
        with contextlib.suppress(Exception):
            proc.wait(timeout=5)
        raise assignment_error
    try:
        _resume_process(handle, proc.pid)
    except BaseException as resume_error:
        try:
            terminate_job(job)
            wait_until_empty(job, timeout=5.0)
        except BaseException as cleanup_error:
            resume_error.add_note(f"job cleanup: {cleanup_error}")
        raise


def close_job(job: int | None) -> None:
    """Close the sole job handle; live members are synchronously signalled."""

    if not _WINDOWS or job is None:
        return
    if job == 0:
        raise WinJobError("Windows job handle is required")
    _close_handle(job, operation="CloseHandle(job)")
