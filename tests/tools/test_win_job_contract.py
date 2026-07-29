"""Platform-independent teeth for the fail-closed Windows Job contract."""

from __future__ import annotations

import ctypes
from types import SimpleNamespace

import pytest

from tools import win_job
from tools.memory_guard_core import payloads


@pytest.fixture(autouse=True)
def _exercise_windows_contract(monkeypatch) -> None:
    monkeypatch.setattr(win_job, "_WINDOWS", True)
    monkeypatch.setattr(
        win_job,
        "_win32_error",
        lambda operation, **_kwargs: win_job.WinJobError(f"{operation} failed"),
    )


def test_assignment_failure_terminates_child_while_suspended(monkeypatch) -> None:
    calls: list[str] = []

    class Kernel32:
        def AssignProcessToJobObject(self, _job, _process) -> int:
            calls.append("assign")
            return 0

        def TerminateProcess(self, _process, _exit_code) -> int:
            calls.append("terminate_process")
            return 1

    proc = SimpleNamespace(
        _handle=111, pid=222, wait=lambda timeout: calls.append("wait")
    )
    monkeypatch.setattr(win_job, "_k32", lambda: Kernel32())
    monkeypatch.setattr(
        win_job,
        "_resume_process",
        lambda _handle, _pid: calls.append("resume"),
    )

    with pytest.raises(win_job.WinJobError, match="AssignProcessToJobObject"):
        win_job.assign_and_resume(333, proc)

    assert calls == ["assign", "terminate_process", "wait"]


def test_resume_failure_terminates_and_drains_assigned_job(monkeypatch) -> None:
    calls: list[str] = []

    class Kernel32:
        def AssignProcessToJobObject(self, _job, _process) -> int:
            calls.append("assign")
            return 1

    proc = SimpleNamespace(_handle=111, pid=222)
    monkeypatch.setattr(win_job, "_k32", lambda: Kernel32())

    def fail_resume(_handle: int, _pid: int) -> None:
        calls.append("resume")
        raise win_job.WinJobError("forced resume failure")

    monkeypatch.setattr(win_job, "_resume_process", fail_resume)
    monkeypatch.setattr(
        win_job,
        "terminate_job",
        lambda _job: calls.append("terminate_job"),
    )
    monkeypatch.setattr(
        win_job,
        "wait_until_empty",
        lambda _job, *, timeout: calls.append(f"wait_empty:{timeout}"),
    )

    with pytest.raises(win_job.WinJobError, match="forced resume failure"):
        win_job.assign_and_resume(333, proc)

    assert calls == ["assign", "resume", "terminate_job", "wait_empty:5.0"]


def test_process_ids_grows_query_buffer_without_fixed_ceiling(monkeypatch) -> None:
    capacities: list[int] = []

    class Kernel32:
        def QueryInformationJobObject(
            self,
            _job,
            _info_class,
            info_pointer,
            _size,
            _returned,
        ) -> int:
            info = info_pointer._obj
            capacity = len(info.ProcessIdList)
            capacities.append(capacity)
            if len(capacities) == 1:
                info.NumberOfAssignedProcesses = 33
                return 0
            info.NumberOfProcessIdsInList = 3
            info.ProcessIdList[0] = 101
            info.ProcessIdList[1] = 202
            info.ProcessIdList[2] = 303
            return 1

    monkeypatch.setattr(win_job, "_k32", lambda: Kernel32())
    monkeypatch.setattr(ctypes, "get_last_error", lambda: 234, raising=False)

    assert win_job.process_ids(444) == (101, 202, 303)
    assert capacities == [16, 33]


def test_complete_job_custody_terminates_and_waits_for_exact_members(
    monkeypatch,
) -> None:
    calls: list[str] = []
    accounting = iter(
        (
            win_job.WindowsJobAccounting(8, 3, 5, 4096, 100, 200, 300),
            win_job.WindowsJobAccounting(8, 0, 8, 4096, 400, 500, 600),
        )
    )
    resources = iter(
        (
            win_job.WindowsSystemResources(100, 800, 4000, 30, 1, 2, 2, 3, 1),
            win_job.WindowsSystemResources(97, 770, 3900, 30, 1, 2, 2, 3, 1),
        )
    )
    monkeypatch.setattr(win_job, "job_accounting", lambda _job: next(accounting))
    monkeypatch.setattr(win_job, "system_resources", lambda: next(resources))
    monkeypatch.setattr(
        win_job,
        "terminate_job",
        lambda _job: calls.append("terminate"),
    )
    monkeypatch.setattr(
        win_job,
        "wait_until_empty",
        lambda _job, *, timeout: calls.append(f"wait:{timeout}"),
    )

    cleanup = win_job.complete_job_custody(777, timeout=2.5)

    assert cleanup is not None
    assert cleanup.completed
    assert cleanup.terminated_remaining_processes
    assert cleanup.before.active_processes == 3
    assert cleanup.after.active_processes == 0
    assert calls == ["terminate", "wait:2.5"]


def test_complete_job_custody_does_not_terminate_an_empty_job(monkeypatch) -> None:
    empty = win_job.WindowsJobAccounting(1, 0, 1, 2048, 100, 200, 300)
    resources = win_job.WindowsSystemResources(100, 800, 4000, 30, 1, 2, 2, 3, 1)
    calls: list[str] = []
    monkeypatch.setattr(win_job, "job_accounting", lambda _job: empty)
    monkeypatch.setattr(win_job, "system_resources", lambda: resources)
    monkeypatch.setattr(
        win_job,
        "terminate_job",
        lambda _job: calls.append("terminate"),
    )
    monkeypatch.setattr(
        win_job,
        "wait_until_empty",
        lambda _job, *, timeout: calls.append(f"wait:{timeout}"),
    )

    cleanup = win_job.complete_job_custody(777, timeout=1.0)

    assert cleanup is not None
    assert cleanup.completed
    assert not cleanup.terminated_remaining_processes
    assert calls == ["wait:1.0"]


def test_job_accounting_payload_serializes_cpu_and_page_fault_totals() -> None:
    accounting = win_job.WindowsJobAccounting(
        total_processes=8,
        active_processes=3,
        total_terminated_processes=5,
        peak_job_commit_bytes=4096,
        total_user_time_100ns=12_500_000,
        total_kernel_time_100ns=7_500_000,
        total_page_fault_count=321,
    )

    assert payloads._windows_job_accounting_payload(accounting) == {
        "total_processes": 8,
        "active_processes": 3,
        "total_terminated_processes": 5,
        "peak_job_commit_bytes": 4096,
        "total_user_time_100ns": 12_500_000,
        "total_kernel_time_100ns": 7_500_000,
        "total_cpu_seconds": 2.0,
        "total_page_fault_count": 321,
    }


def test_system_resources_converts_page_counts_without_open_handles(
    monkeypatch,
) -> None:
    class Psapi:
        def GetPerformanceInfo(self, info_pointer, _size) -> int:
            info = info_pointer._obj
            info.PageSize = 4096
            info.CommitTotal = 10
            info.CommitLimit = 20
            info.CommitPeak = 15
            info.PhysicalTotal = 30
            info.PhysicalAvailable = 12
            info.HandleCount = 400
            info.ProcessCount = 50
            info.ThreadCount = 600
            return 1

    class Kernel32:
        def GetCurrentProcess(self) -> int:
            return 999

        def GetProcessHandleCount(self, _process, count_pointer) -> int:
            count_pointer._obj.value = 33
            return 1

    monkeypatch.setattr(win_job, "_psapi", lambda: Psapi())
    monkeypatch.setattr(win_job, "_k32", lambda: Kernel32())

    resources = win_job.system_resources()

    assert resources.process_count == 50
    assert resources.thread_count == 600
    assert resources.system_handle_count == 400
    assert resources.guard_handle_count == 33
    assert resources.commit_total_bytes == 10 * 4096
    assert resources.commit_limit_bytes == 20 * 4096
    assert resources.commit_peak_bytes == 15 * 4096
    assert resources.physical_total_bytes == 30 * 4096
    assert resources.physical_available_bytes == 12 * 4096
    assert resources.errors == ()


@pytest.mark.parametrize("operation", ["terminate", "close"])
def test_job_operation_errors_are_surfaced(monkeypatch, operation: str) -> None:
    class Kernel32:
        def TerminateJobObject(self, _job, _exit_code) -> int:
            return 0

        def CloseHandle(self, _job) -> int:
            return 0

    monkeypatch.setattr(win_job, "_k32", lambda: Kernel32())

    with pytest.raises(win_job.WinJobError, match="failed"):
        if operation == "terminate":
            win_job.terminate_job(555)
        else:
            win_job.close_job(555)
