"""Platform-independent teeth for the fail-closed Windows Job contract."""

from __future__ import annotations

import ctypes
from types import SimpleNamespace

import pytest

from tools import win_job


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

    proc = SimpleNamespace(_handle=111, pid=222, wait=lambda timeout: calls.append("wait"))
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
