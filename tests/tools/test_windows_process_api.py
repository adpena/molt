from __future__ import annotations

import ctypes
import sys
from ctypes import wintypes
from types import SimpleNamespace

import pytest

from molt import windows_process_api
from tools.proof_queue_pkg import custody
from tests.process_guard_common import install_module_view


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

    @ctypes.CFUNCTYPE(wintypes.BOOL, wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD))
    def exit_code(handle, code):
        observed.append(("exit", handle))
        code.contents.value = 259
        return 1

    @ctypes.CFUNCTYPE(wintypes.HANDLE)
    def current_process():
        return handle_value

    api = SimpleNamespace(
        OpenProcess=open_process,
        CloseHandle=close_handle,
        GetProcessTimes=process_times,
        GetExitCodeProcess=exit_code,
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
        assert custody._pid_alive(123)
        assert windows_process_api.process_query_api() is api
        assert loads == [("kernel32", {"use_last_error": True})]
        assert observed == [
            ("times", handle_value),
            ("close", handle_value),
            ("exit", handle_value),
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
