from __future__ import annotations

import ctypes
import sys
from ctypes import wintypes
from types import SimpleNamespace

import pytest

from molt import process_model, windows_process_api
from tools.proof_queue_pkg import custody
from tests.process_guard_common import install_module_view


@pytest.mark.parametrize(
    ("pid", "valid"),
    [
        (1, True),
        (2**31, True),
        (0xFFFFFFFF, True),
        (0, False),
        (-1, False),
        (True, False),
        (1.0, False),
        (0x100000000, False),
    ],
)
def test_a_windows_pid_is_a_positive_int_within_a_dword(pid, valid) -> None:
    assert windows_process_api.valid_windows_pid(pid) is valid


def test_process_query_api_rejects_non_windows_before_loading(monkeypatch) -> None:
    windows_process_api.process_query_api.cache_clear()
    install_module_view(monkeypatch, "sys", sys, windows_process_api, platform="linux")
    with pytest.raises(OSError, match="require Windows"):
        windows_process_api.process_query_api()


@pytest.mark.skipif(sys.platform != "win32", reason="Win32 ctypes ABI")
def test_queue_process_queries_preserve_pointer_width_and_close_handles(
    monkeypatch,
) -> None:
    handle_value = (
        0x1234567887654321 if ctypes.sizeof(ctypes.c_void_p) == 8 else 0x12345678
    )
    observed = []
    creation_ticks = [(0x1234 << 32) | 0x5678]

    @ctypes.CFUNCTYPE(ctypes.c_void_p, wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
    def open_process(_access, _inherit, _pid):
        return handle_value

    # Model WinDLL's unbound integer default: the shared binding must correct it.
    open_process.restype = ctypes.c_int

    @ctypes.CFUNCTYPE(wintypes.BOOL, wintypes.HANDLE)
    def close_handle(handle):
        observed.append(("close", handle))
        return 1

    @ctypes.CFUNCTYPE(
        wintypes.BOOL,
        wintypes.HANDLE,
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
    )
    def process_times(handle, created, _exited, _kernel, _user):
        observed.append(("times", handle))
        created.contents.dwHighDateTime = creation_ticks[0] >> 32
        created.contents.dwLowDateTime = creation_ticks[0] & 0xFFFFFFFF
        return 1

    @ctypes.CFUNCTYPE(wintypes.HANDLE)
    def current_process():
        return handle_value

    @ctypes.CFUNCTYPE(wintypes.DWORD, wintypes.HANDLE, wintypes.DWORD)
    def wait_process(handle, timeout):
        observed.append(("wait", handle, timeout))
        return 0x102

    api = SimpleNamespace(
        OpenProcess=open_process,
        WaitForSingleObject=wait_process,
        CloseHandle=close_handle,
        GetProcessTimes=process_times,
        GetCurrentProcess=current_process,
    )
    loads = []

    def load(name, **kwargs):
        loads.append((name, kwargs))
        return api

    windows_process_api.process_query_api.cache_clear()
    monkeypatch.setattr(ctypes, "WinDLL", load)
    try:
        assert custody._windows_process_creation_ticks(123) == (0x1234 << 32) | 0x5678
        assert process_model.pid_alive(123)
        assert windows_process_api.process_query_api() is api
        assert loads == [("kernel32", {"use_last_error": True})]
        assert observed == [
            ("times", handle_value),
            ("close", handle_value),
            ("wait", handle_value, 0),
            ("close", handle_value),
        ]
        # A fresh WinDLL defaults undeclared parameters to C int. Retain that
        # default at the consumer boundary, so omitting the shared binder must
        # fail for a pointer-width process handle.
        windows_process_api.process_query_api.cache_clear()
        process_times.argtypes = None
        unix_ns = 1_700_000_000_123_456_700
        creation_ticks[0] = 116_444_736_000_000_000 + unix_ns // 100
        from molt.process_birth import current_process_started_ns

        assert current_process_started_ns() == unix_ns
        assert observed[-1] == ("times", handle_value)
        assert len(observed) == 5, "a current-process pseudo handle is not closed"
    finally:
        windows_process_api.process_query_api.cache_clear()


def test_process_liveness_is_query_only_and_unknown_is_not_death(monkeypatch):
    observations = []
    outcomes = iter([0x102, 0, 0xFFFFFFFF, 0x80])
    handle = 0x1234567887654321

    def open_process(access, inherit, pid):
        observations.append(("open", access, inherit, pid))
        return handle

    def wait(process, timeout):
        assert process == handle and timeout == 0
        return next(outcomes)

    def close_process(process):
        observations.append(("close", process))
        return 1

    api = SimpleNamespace(
        OpenProcess=open_process,
        WaitForSingleObject=wait,
        CloseHandle=close_process,
    )
    install_module_view(monkeypatch, "sys", sys, windows_process_api, platform="win32")
    install_module_view(
        monkeypatch,
        "ctypes",
        ctypes,
        windows_process_api,
        get_last_error=lambda: 5,
        WinError=lambda error: OSError(f"WinError {error}"),
    )
    monkeypatch.setattr(windows_process_api, "process_query_api", lambda: api)
    assert windows_process_api.process_is_alive(123) is True
    assert windows_process_api.process_is_alive(123) is False
    with pytest.raises(OSError, match="WinError 5"):
        windows_process_api.process_is_alive(123)
    with pytest.raises(OSError, match="unexpected Windows process wait"):
        windows_process_api.process_is_alive(123)
    assert observations == [("open", 0x100000, False, 123), ("close", handle)] * 4
    for invalid in (0, -1, True, 2**32):
        with pytest.raises(OSError, match="invalid positive"):
            windows_process_api.process_is_alive(invalid)
    assert len(observations) == 8
    api.OpenProcess = lambda *args: 0
    with pytest.raises(OSError, match="WinError 5"):
        windows_process_api.process_is_alive(123)
    install_module_view(
        monkeypatch,
        "ctypes",
        ctypes,
        windows_process_api,
        get_last_error=lambda: 87,
        WinError=lambda error: OSError(f"WinError {error}"),
    )
    assert windows_process_api.process_is_alive(123) is False
    assert len(observations) == 8


@pytest.mark.skipif(
    sys.platform != "win32", reason="actual Windows process-handle query"
)
def test_process_liveness_native_self_absent_exit259_and_query_failure(
    tmp_path, monkeypatch
):
    import os
    from tools.command_execution import CommandExecutor

    api = windows_process_api.process_query_api()
    assert windows_process_api.process_is_alive(os.getpid()) is True
    assert windows_process_api.process_is_alive(2_000_000_000) is False
    release = tmp_path / "release"
    command = [
        sys.executable,
        "-I",
        "-S",
        "-c",
        "import pathlib,sys,time; p=pathlib.Path(sys.argv[1]); deadline=time.monotonic()+20\n"
        "while not p.exists() and time.monotonic()<deadline: time.sleep(.01)\n"
        "assert p.exists(); sys.exit(259)",
        str(release),
    ]
    proc = CommandExecutor.for_file(__file__).start_owned(command)
    try:
        assert windows_process_api.process_is_alive(proc.pid) is True
        release.touch()
        assert proc.wait(timeout=10) == 259
        # Popen retains its handle: the terminated process object still exists.
        assert windows_process_api.process_is_alive(proc.pid) is False
    finally:
        release.touch()
        if proc.poll() is None:
            proc.terminate()
        proc.wait(timeout=5)

    closed = []

    def failed_wait(handle, timeout):
        ctypes.set_last_error(6)
        return 0xFFFFFFFF

    def close(handle):
        closed.append(handle)
        return api.CloseHandle(handle)

    monkeypatch.setattr(
        windows_process_api,
        "process_query_api",
        lambda: SimpleNamespace(
            OpenProcess=api.OpenProcess,
            WaitForSingleObject=failed_wait,
            CloseHandle=close,
        ),
    )
    with pytest.raises(OSError) as caught:
        windows_process_api.process_is_alive(os.getpid())
    assert caught.value.winerror == 6
    assert len(closed) == 1
    assert proc.returncode == 259


@pytest.mark.parametrize(
    "observation,expected",
    [
        (True, True),
        (False, False),
        (PermissionError, True),
        (ProcessLookupError, True),
        (OSError, True),
    ],
)
def test_queue_liveness_retains_unknown_native_query(
    monkeypatch, observation, expected
):
    import os

    queries = []

    def query(pid):
        queries.append(pid)
        if isinstance(observation, type):
            raise observation("native query unavailable")
        return observation

    def forbidden_signal(*args):
        pytest.fail("Windows queue liveness must never signal")

    install_module_view(
        monkeypatch, "os", os, custody, name="nt", kill=forbidden_signal
    )
    monkeypatch.setattr(custody.process_model, "pid_alive", query)
    assert custody._guard_process_live(123, None) is expected
    assert queries == [123]
    # A corrupt pid proves no exit: the row keeps its run, and nothing is asked.
    for corrupt in (0, -5, 2**32, 2**63):
        assert custody._guard_process_live(corrupt, None)
    assert queries == [123]


@pytest.mark.parametrize("wait_result", [0, 0x102, 0xFFFFFFFF])
def test_process_query_failed_close_never_hides_primary_error(monkeypatch, wait_result):
    observed = []
    last_error = [0]
    wait_error = OSError("independent native wait failure")
    close_error = OSError("independent native close failure")

    def opened(access, inherit, pid):
        assert (access, inherit, pid) == (0x100000, False, 123)
        observed.append("open")
        return 0x1234567887654321

    def waited(handle, timeout):
        assert (handle, timeout) == (0x1234567887654321, 0)
        observed.append("wait")
        last_error[0] = 5
        return wait_result

    def failed_close(handle):
        assert handle == 0x1234567887654321
        observed.append("close")
        last_error[0] = 6
        return 0

    install_module_view(monkeypatch, "sys", sys, windows_process_api, platform="win32")
    install_module_view(
        monkeypatch,
        "ctypes",
        ctypes,
        windows_process_api,
        get_last_error=lambda: last_error[0],
        WinError=lambda error: {5: wait_error, 6: close_error}[error],
    )
    monkeypatch.setattr(
        windows_process_api,
        "process_query_api",
        lambda: SimpleNamespace(
            OpenProcess=opened, WaitForSingleObject=waited, CloseHandle=failed_close
        ),
    )
    with pytest.raises(OSError) as caught:
        windows_process_api.process_is_alive(123)
    if wait_result == 0xFFFFFFFF:
        assert caught.value is wait_error
        assert any("close failure" in note for note in caught.value.__notes__)
    else:
        assert caught.value is close_error
    assert observed == ["open", "wait", "close"]
