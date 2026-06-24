from __future__ import annotations

import os
import time


WINDOWS_FULL_COMMAND_LINE_EXECUTABLE_NAMES = frozenset(
    {
        "cargo.exe",
        "clang.exe",
        "clang-cl.exe",
        "lld-link.exe",
        "molt-backend.exe",
        "node.exe",
        "python.exe",
        "pythonw.exe",
        "py.exe",
        "rustc.exe",
        "uv.exe",
        "zig.exe",
    }
)


def _windows_process_needs_full_command_line(exe_name: str) -> bool:
    return exe_name.strip().casefold() in WINDOWS_FULL_COMMAND_LINE_EXECUTABLE_NAMES


def _filetime_to_unix_seconds(low: int, high: int) -> float | None:
    ticks = (high << 32) | low
    if ticks <= 0:
        return None
    return (ticks - 116444736000000000) / 10_000_000


def _windows_process_snapshot_rows() -> list[tuple[int, int, int, str, int | None, int | None]]:
    if os.name != "nt":
        return []
    import ctypes
    from ctypes import wintypes

    class PROCESSENTRY32W(ctypes.Structure):
        _fields_ = [
            ("dwSize", wintypes.DWORD),
            ("cntUsage", wintypes.DWORD),
            ("th32ProcessID", wintypes.DWORD),
            ("th32DefaultHeapID", ctypes.c_size_t),
            ("th32ModuleID", wintypes.DWORD),
            ("cntThreads", wintypes.DWORD),
            ("th32ParentProcessID", wintypes.DWORD),
            ("pcPriClassBase", wintypes.LONG),
            ("dwFlags", wintypes.DWORD),
            ("szExeFile", wintypes.WCHAR * 260),
        ]

    class PROCESS_MEMORY_COUNTERS(ctypes.Structure):
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
        ]

    class PROCESS_BASIC_INFORMATION(ctypes.Structure):
        _fields_ = [
            ("Reserved1", ctypes.c_void_p),
            ("PebBaseAddress", ctypes.c_void_p),
            ("Reserved2", ctypes.c_void_p * 2),
            ("UniqueProcessId", ctypes.c_size_t),
            ("InheritedFromUniqueProcessId", ctypes.c_size_t),
        ]

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    ntdll = ctypes.WinDLL("ntdll", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    create_snapshot = kernel32.CreateToolhelp32Snapshot
    create_snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
    create_snapshot.restype = wintypes.HANDLE
    process_first = kernel32.Process32FirstW
    process_first.argtypes = [wintypes.HANDLE, ctypes.POINTER(PROCESSENTRY32W)]
    process_first.restype = wintypes.BOOL
    process_next = kernel32.Process32NextW
    process_next.argtypes = [wintypes.HANDLE, ctypes.POINTER(PROCESSENTRY32W)]
    process_next.restype = wintypes.BOOL
    close_handle = kernel32.CloseHandle
    close_handle.argtypes = [wintypes.HANDLE]
    close_handle.restype = wintypes.BOOL
    open_process = kernel32.OpenProcess
    open_process.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    open_process.restype = wintypes.HANDLE
    get_process_memory_info = psapi.GetProcessMemoryInfo
    get_process_memory_info.argtypes = [
        wintypes.HANDLE,
        ctypes.POINTER(PROCESS_MEMORY_COUNTERS),
        wintypes.DWORD,
    ]
    get_process_memory_info.restype = wintypes.BOOL
    get_process_times = kernel32.GetProcessTimes
    get_process_times.argtypes = [
        wintypes.HANDLE,
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
    ]
    get_process_times.restype = wintypes.BOOL
    query_full_process_image_name = kernel32.QueryFullProcessImageNameW
    query_full_process_image_name.argtypes = [
        wintypes.HANDLE,
        wintypes.DWORD,
        wintypes.LPWSTR,
        ctypes.POINTER(wintypes.DWORD),
    ]
    query_full_process_image_name.restype = wintypes.BOOL
    read_process_memory = kernel32.ReadProcessMemory
    read_process_memory.argtypes = [
        wintypes.HANDLE,
        wintypes.LPCVOID,
        wintypes.LPVOID,
        ctypes.c_size_t,
        ctypes.POINTER(ctypes.c_size_t),
    ]
    read_process_memory.restype = wintypes.BOOL
    nt_query_information_process = ntdll.NtQueryInformationProcess
    nt_query_information_process.argtypes = [
        wintypes.HANDLE,
        wintypes.ULONG,
        ctypes.c_void_p,
        wintypes.ULONG,
        ctypes.POINTER(wintypes.ULONG),
    ]
    nt_query_information_process.restype = wintypes.LONG

    TH32CS_SNAPPROCESS = 0x00000002
    PROCESS_QUERY_INFORMATION = 0x0400
    PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
    PROCESS_VM_READ = 0x0010
    INVALID_HANDLE_VALUE = wintypes.HANDLE(-1).value
    ProcessBasicInformation = 0
    pointer_size = ctypes.sizeof(ctypes.c_void_p)
    peb_process_parameters_offset = 0x20 if pointer_size == 8 else 0x10
    command_line_offset = 0x70 if pointer_size == 8 else 0x40
    command_line_buffer_offset = command_line_offset + (8 if pointer_size == 8 else 4)

    def read_memory(handle: wintypes.HANDLE, address: int, size: int) -> bytes | None:
        if address <= 0 or size <= 0:
            return None
        buffer = (ctypes.c_ubyte * size)()
        bytes_read = ctypes.c_size_t(0)
        if not read_process_memory(
            handle,
            ctypes.c_void_p(address),
            buffer,
            size,
            ctypes.byref(bytes_read),
        ):
            return None
        if bytes_read.value <= 0:
            return None
        return bytes(buffer[: bytes_read.value])

    def read_u16(handle: wintypes.HANDLE, address: int) -> int | None:
        raw = read_memory(handle, address, 2)
        if raw is None or len(raw) != 2:
            return None
        return int.from_bytes(raw, "little", signed=False)

    def read_ptr(handle: wintypes.HANDLE, address: int) -> int | None:
        raw = read_memory(handle, address, pointer_size)
        if raw is None or len(raw) != pointer_size:
            return None
        return int.from_bytes(raw, "little", signed=False)

    def read_process_command_line(handle: wintypes.HANDLE) -> str | None:
        info = PROCESS_BASIC_INFORMATION()
        returned = wintypes.ULONG(0)
        status = nt_query_information_process(
            handle,
            ProcessBasicInformation,
            ctypes.byref(info),
            ctypes.sizeof(info),
            ctypes.byref(returned),
        )
        if status != 0 or not info.PebBaseAddress:
            return None
        process_parameters = read_ptr(
            handle,
            int(info.PebBaseAddress) + peb_process_parameters_offset,
        )
        if not process_parameters:
            return None
        byte_len = read_u16(handle, process_parameters + command_line_offset)
        buffer_addr = read_ptr(handle, process_parameters + command_line_buffer_offset)
        if not byte_len or not buffer_addr:
            return None
        raw = read_memory(handle, buffer_addr, min(byte_len, 32768))
        if raw is None:
            return None
        return raw.decode("utf-16-le", errors="replace").strip("\x00")

    def read_process_image_name(handle: wintypes.HANDLE) -> str | None:
        size = wintypes.DWORD(32768)
        buffer = ctypes.create_unicode_buffer(size.value)
        if query_full_process_image_name(handle, 0, buffer, ctypes.byref(size)):
            return buffer.value
        return None

    snapshot = create_snapshot(TH32CS_SNAPPROCESS, 0)
    if snapshot == INVALID_HANDLE_VALUE:
        return []
    rows: list[tuple[int, int, int, str, int | None, int | None]] = []
    try:
        entry = PROCESSENTRY32W()
        entry.dwSize = ctypes.sizeof(PROCESSENTRY32W)
        ok = process_first(snapshot, ctypes.byref(entry))
        now = time.time()
        while ok:
            pid = int(entry.th32ProcessID)
            if pid > 0:
                rss_kb = 0
                elapsed_sec: int | None = None
                started_at_ns: int | None = None
                exe_name = str(entry.szExeFile).strip()
                command = exe_name
                access_masks = (
                    (PROCESS_QUERY_INFORMATION | PROCESS_VM_READ),
                    (PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ),
                    PROCESS_QUERY_LIMITED_INFORMATION,
                )
                handle = None
                for access in access_masks:
                    handle = open_process(access, False, pid)
                    if handle:
                        break
                if handle:
                    try:
                        image_name = read_process_image_name(handle)
                        if _windows_process_needs_full_command_line(exe_name):
                            command = (
                                read_process_command_line(handle)
                                or image_name
                                or command
                            )
                        else:
                            command = image_name or command
                        counters = PROCESS_MEMORY_COUNTERS()
                        counters.cb = ctypes.sizeof(PROCESS_MEMORY_COUNTERS)
                        if get_process_memory_info(
                            handle,
                            ctypes.byref(counters),
                            counters.cb,
                        ):
                            rss_kb = max(
                                0,
                                int((counters.WorkingSetSize + 1023) // 1024),
                            )
                        created = wintypes.FILETIME()
                        exited = wintypes.FILETIME()
                        kernel = wintypes.FILETIME()
                        user = wintypes.FILETIME()
                        if get_process_times(
                            handle,
                            ctypes.byref(created),
                            ctypes.byref(exited),
                            ctypes.byref(kernel),
                            ctypes.byref(user),
                        ):
                            created_ts = _filetime_to_unix_seconds(
                                int(created.dwLowDateTime),
                                int(created.dwHighDateTime),
                            )
                            if created_ts is not None:
                                elapsed_sec = max(0, int(now - created_ts))
                                started_at_ns = max(
                                    0,
                                    int(created_ts * 1_000_000_000),
                                )
                    finally:
                        close_handle(handle)
                rows.append(
                    (
                        pid,
                        int(entry.th32ParentProcessID),
                        rss_kb,
                        command,
                        elapsed_sec,
                        started_at_ns,
                    )
                )
            ok = process_next(snapshot, ctypes.byref(entry))
    finally:
        close_handle(snapshot)
    return rows
