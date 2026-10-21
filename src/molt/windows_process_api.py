"""Pointer-width-correct Win32 process queries for compiler and development tools."""

from __future__ import annotations

import ctypes
import sys
from ctypes import wintypes
from functools import lru_cache


def bind_process_query_api(api: ctypes.CDLL) -> None:
    """Bind shared query signatures on a consumer-owned DLL function table."""
    api.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    api.OpenProcess.restype = wintypes.HANDLE
    api.CloseHandle.argtypes = [wintypes.HANDLE]
    api.CloseHandle.restype = wintypes.BOOL
    api.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    api.WaitForSingleObject.restype = wintypes.DWORD
    api.GetCurrentProcess.argtypes = []
    api.GetCurrentProcess.restype = wintypes.HANDLE
    api.GetProcessTimes.argtypes = [
        wintypes.HANDLE,
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
    ]
    api.GetProcessTimes.restype = wintypes.BOOL


@lru_cache(maxsize=1)
def process_query_api() -> ctypes.CDLL:
    """Reuse a query-only table; consumers adding other APIs own their DLL table."""
    if sys.platform != "win32":
        raise OSError("Win32 process queries require Windows")
    api = ctypes.WinDLL("kernel32", use_last_error=True)
    bind_process_query_api(api)
    return api


def process_is_alive(pid: int) -> bool:
    """Observe one Windows PID; raise on unknown instead of claiming absence.

    A zero-time process-handle wait distinguishes actual exit code 259 from
    STILL_ACTIVE. No termination rights, signal, process census or wait delay.
    """
    if sys.platform != "win32":
        raise OSError("Win32 process queries require Windows")
    if type(pid) is not int or not 0 < pid <= 0xFFFFFFFF:
        raise OSError("invalid positive Windows process ID")
    api = process_query_api()
    handle = api.OpenProcess(0x00100000, False, pid)  # SYNCHRONIZE only
    if not handle:
        error = ctypes.get_last_error()
        if error == 87:  # ERROR_INVALID_PARAMETER: admitted positive PID absent
            return False
        raise ctypes.WinError(error)
    primary_error: BaseException | None = None
    try:
        result = api.WaitForSingleObject(handle, 0)
        if result == 0:  # WAIT_OBJECT_0: process terminated
            return False
        if result == 0x102:  # WAIT_TIMEOUT: process still live
            return True
        if result == 0xFFFFFFFF:  # WAIT_FAILED
            raise ctypes.WinError(ctypes.get_last_error())
        raise OSError(f"unexpected Windows process wait result: {result}")
    except BaseException as error:
        primary_error = error
        raise
    finally:
        try:
            if not api.CloseHandle(handle):
                raise ctypes.WinError(ctypes.get_last_error())
        except BaseException as close_error:
            if primary_error is None:
                raise
            primary_error.add_note(
                f"Windows process query handle close failed: {close_error!r}"
            )
