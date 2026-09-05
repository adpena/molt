"""Pointer-width-correct Win32 process query bindings shared by Molt tools."""

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
    api.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
    api.GetExitCodeProcess.restype = wintypes.BOOL


@lru_cache(maxsize=1)
def process_query_api() -> ctypes.CDLL:
    """Reuse a query-only table; consumers adding other APIs own their DLL table."""
    if sys.platform != "win32":
        raise OSError("Win32 process queries require Windows")
    api = ctypes.WinDLL("kernel32", use_last_error=True)
    bind_process_query_api(api)
    return api
