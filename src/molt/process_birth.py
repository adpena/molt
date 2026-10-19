"""The wall-clock time this process started, from the kernel.

A long-lived process may hold bytes it read before any timestamp it could take
itself, so a bound on what it read must come from the kernel's record of its
start. Each reader returns a value no later than the true start, or None when
the platform cannot report it; callers fail closed on None.
"""

from __future__ import annotations

import os
import sys


def current_process_started_ns() -> int | None:
    """Unix-epoch nanoseconds at or before this process's start, or None."""
    try:
        if sys.platform.startswith("linux"):
            return _linux_started_ns()
        if sys.platform == "darwin":
            return _darwin_started_ns()
        if sys.platform == "win32":
            return _windows_started_ns()
    except (OSError, ValueError, AttributeError, IndexError):
        return None
    return None


def _linux_started_ns() -> int | None:
    # proc(5): field 22 of /proc/<pid>/stat is the start time in clock ticks
    # after boot; `btime` in /proc/stat is the boot time in whole seconds,
    # rounded down, so their sum never exceeds the true start.
    with open("/proc/self/stat", "rb") as handle:
        stat_row = handle.read()
    # The command name in field 2 may contain spaces and parentheses.
    fields = stat_row[stat_row.rindex(b")") + 2 :].split()
    ticks = int(fields[22 - 3])
    with open("/proc/stat", "rb") as handle:
        boot_line = next(line for line in handle if line.startswith(b"btime "))
    boot_seconds = int(boot_line.split()[1])
    hertz = os.sysconf("SC_CLK_TCK")
    if hertz <= 0:
        return None
    return boot_seconds * 1_000_000_000 + ticks * 1_000_000_000 // hertz


def _darwin_started_ns() -> int | None:
    import ctypes

    # libproc.h: proc_pidinfo(PROC_PIDTBSDINFO) fills `struct proc_bsdinfo`;
    # its last two fields are pbi_start_tvsec and pbi_start_tvusec.
    proc_pidtbsdinfo = 3
    size = 136
    start_offset = 120
    libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    buffer = ctypes.create_string_buffer(size)
    written = libproc.proc_pidinfo(
        ctypes.c_int(os.getpid()),
        ctypes.c_int(proc_pidtbsdinfo),
        ctypes.c_uint64(0),
        buffer,
        ctypes.c_int(size),
    )
    if written != size:
        return None
    seconds = int.from_bytes(buffer.raw[start_offset : start_offset + 8], "little")
    microseconds = int.from_bytes(
        buffer.raw[start_offset + 8 : start_offset + 16], "little"
    )
    return seconds * 1_000_000_000 + microseconds * 1_000


def _windows_started_ns() -> int | None:
    if sys.platform != "win32":
        return None
    import ctypes
    from ctypes import wintypes
    from molt.windows_process_api import process_query_api

    kernel32 = process_query_api()
    creation = wintypes.FILETIME()
    unused = (wintypes.FILETIME(), wintypes.FILETIME(), wintypes.FILETIME())
    if not kernel32.GetProcessTimes(
        kernel32.GetCurrentProcess(),
        ctypes.byref(creation),
        *(ctypes.byref(value) for value in unused),
    ):
        return None
    # FILETIME counts 100 ns intervals from 1601-01-01.
    ticks = (creation.dwHighDateTime << 32) | creation.dwLowDateTime
    unix_ticks = ticks - 116_444_736_000_000_000
    return unix_ticks * 100 if unix_ticks > 0 else None
