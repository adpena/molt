from __future__ import annotations

from collections.abc import Mapping
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from functools import lru_cache
from types import SimpleNamespace


DEFAULT_WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_SEC = 5.0
WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_ENV = "MOLT_WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_SEC"
WINDOWS_PROCESS_SNAPSHOT_HELPER_ARG = "--molt-windows-process-snapshot-json"
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


class ProcessSnapshotError(RuntimeError):
    """Raised when a process-table snapshot is not authoritative."""


class WindowsProcessSnapshotTimeout(ProcessSnapshotError, TimeoutError):
    """Raised when Windows process-table custody cannot be sampled completely."""


def _windows_process_snapshot_timeout_sec(
    env: Mapping[str, str] | None = None,
) -> float | None:
    source = os.environ if env is None else env
    raw = source.get(WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_ENV, "").strip()
    if not raw:
        return DEFAULT_WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_SEC
    lowered = raw.casefold()
    if lowered in {"0", "false", "off", "no"}:
        return None
    try:
        parsed = float(raw)
    except ValueError:
        return DEFAULT_WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_SEC
    if parsed <= 0:
        return None
    return parsed


def _coerce_windows_process_snapshot_rows(
    payload: object,
) -> list[tuple[int, int, int, str, int | None, int | None]]:
    if not isinstance(payload, list):
        raise ValueError("Windows process snapshot payload must be a list")
    rows: list[tuple[int, int, int, str, int | None, int | None]] = []
    for row in payload:
        if not isinstance(row, list) or len(row) != 6:
            raise ValueError("Windows process snapshot row must have six fields")
        pid, ppid, rss_kb, command, elapsed_sec, started_at_ns = row
        if not (
            isinstance(pid, int)
            and isinstance(ppid, int)
            and isinstance(rss_kb, int)
            and isinstance(command, str)
        ):
            raise ValueError("Windows process snapshot row has invalid field types")
        if elapsed_sec is not None and not isinstance(elapsed_sec, int):
            raise ValueError("Windows process snapshot elapsed_sec must be int or null")
        if started_at_ns is not None and not isinstance(started_at_ns, int):
            raise ValueError(
                "Windows process snapshot started_at_ns must be int or null"
            )
        rows.append((pid, ppid, rss_kb, command, elapsed_sec, started_at_ns))
    return rows


def _windows_process_snapshot_rows_hard_timeout() -> list[
    tuple[int, int, int, str, int | None, int | None]
]:
    if os.name != "nt":
        return []
    timeout_sec = _windows_process_snapshot_timeout_sec()
    creationflags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    try:
        result = subprocess.run(
            [
                sys.executable,
                str(Path(__file__).resolve()),
                WINDOWS_PROCESS_SNAPSHOT_HELPER_ARG,
            ],
            capture_output=True,
            text=True,
            timeout=timeout_sec,
            check=False,
            creationflags=creationflags,
        )
    except subprocess.TimeoutExpired as exc:
        raise WindowsProcessSnapshotTimeout(
            f"Windows process snapshot helper exceeded {timeout_sec:.3f}s"
            if timeout_sec is not None
            else "Windows process snapshot helper timed out"
        ) from exc
    except OSError as exc:
        raise ProcessSnapshotError(
            f"Windows process snapshot helper could not start: {exc}"
        ) from exc
    if result.returncode != 0:
        raise ProcessSnapshotError(
            "Windows process snapshot helper failed with "
            f"exit code {result.returncode}: {result.stderr.strip()}"
        )
    try:
        payload = json.loads(result.stdout)
        return _coerce_windows_process_snapshot_rows(payload)
    except (json.JSONDecodeError, ValueError, TypeError) as exc:
        raise ProcessSnapshotError(
            f"Windows process snapshot helper returned invalid payload: {exc}"
        ) from exc


def _windows_process_needs_full_command_line(exe_name: str) -> bool:
    return exe_name.strip().casefold() in WINDOWS_FULL_COMMAND_LINE_EXECUTABLE_NAMES


def _windows_process_memory_counters_type(ctypes_module, wintypes_module):
    class PROCESS_MEMORY_COUNTERS(ctypes_module.Structure):
        _fields_ = [
            ("cb", wintypes_module.DWORD),
            ("PageFaultCount", wintypes_module.DWORD),
            ("PeakWorkingSetSize", ctypes_module.c_size_t),
            ("WorkingSetSize", ctypes_module.c_size_t),
            ("QuotaPeakPagedPoolUsage", ctypes_module.c_size_t),
            ("QuotaPagedPoolUsage", ctypes_module.c_size_t),
            ("QuotaPeakNonPagedPoolUsage", ctypes_module.c_size_t),
            ("QuotaNonPagedPoolUsage", ctypes_module.c_size_t),
            ("PagefileUsage", ctypes_module.c_size_t),
            ("PeakPagefileUsage", ctypes_module.c_size_t),
        ]

    return PROCESS_MEMORY_COUNTERS


def _windows_get_process_memory_info(
    psapi,
    ctypes_module,
    wintypes_module,
    process_memory_counters_type,
):
    get_process_memory_info = psapi.GetProcessMemoryInfo
    get_process_memory_info.argtypes = [
        wintypes_module.HANDLE,
        ctypes_module.POINTER(process_memory_counters_type),
        wintypes_module.DWORD,
    ]
    get_process_memory_info.restype = wintypes_module.BOOL
    return get_process_memory_info


def _working_set_rss_kb(counters) -> int:
    return max(0, int((counters.WorkingSetSize + 1023) // 1024))


def windows_process_handle_rss_kb(handle: object) -> int | None:
    if os.name != "nt" or not handle:
        return None
    try:
        handle_value = int(handle)
    except (TypeError, ValueError):
        return None

    try:
        import ctypes
        from ctypes import wintypes

        process_memory_counters_type = _windows_process_memory_counters_type(
            ctypes,
            wintypes,
        )
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        get_process_memory_info = _windows_get_process_memory_info(
            psapi,
            ctypes,
            wintypes,
            process_memory_counters_type,
        )
        counters = process_memory_counters_type()
        counters.cb = ctypes.sizeof(process_memory_counters_type)
        if not get_process_memory_info(
            wintypes.HANDLE(handle_value),
            ctypes.byref(counters),
            counters.cb,
        ):
            return None
    except (AttributeError, OSError, TypeError, ValueError):
        return None
    return _working_set_rss_kb(counters)


def windows_process_handle_started_at_ns(handle: object) -> int | None:
    """Read the stable creation marker from the already-owned process handle."""

    if os.name != "nt" or not handle:
        return None
    try:
        handle_value = int(handle)
    except (TypeError, ValueError):
        return None
    try:
        import ctypes
        from ctypes import wintypes

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        get_process_times = kernel32.GetProcessTimes
        get_process_times.argtypes = [
            wintypes.HANDLE,
            ctypes.POINTER(wintypes.FILETIME),
            ctypes.POINTER(wintypes.FILETIME),
            ctypes.POINTER(wintypes.FILETIME),
            ctypes.POINTER(wintypes.FILETIME),
        ]
        get_process_times.restype = wintypes.BOOL
        created = wintypes.FILETIME()
        exited = wintypes.FILETIME()
        kernel = wintypes.FILETIME()
        user = wintypes.FILETIME()
        if not get_process_times(
            wintypes.HANDLE(handle_value),
            ctypes.byref(created),
            ctypes.byref(exited),
            ctypes.byref(kernel),
            ctypes.byref(user),
        ):
            return None
    except (AttributeError, OSError, TypeError, ValueError):
        return None
    return _filetime_to_unix_ns(
        int(created.dwLowDateTime),
        int(created.dwHighDateTime),
    )


def _filetime_to_unix_seconds(low: int, high: int) -> float | None:
    ns = _filetime_to_unix_ns(low, high)
    return None if ns is None else ns / 1_000_000_000


def _filetime_to_unix_ns(low: int, high: int) -> int | None:
    ticks = (high << 32) | low
    unix_100ns = ticks - 116444736000000000
    if unix_100ns <= 0:
        return None
    return unix_100ns * 100


def _validated_windows_process_binding(
    enumerated_pid: int,
    bound_pid: int | None,
    bound_ppid: int | None,
    started_at_ns: int | None,
) -> tuple[int, int | None]:
    """Bind lineage and creation identity to the same opened process handle."""

    if bound_pid != enumerated_pid or bound_ppid is None or started_at_ns is None:
        return 0, None
    return max(0, bound_ppid), started_at_ns


@lru_cache(maxsize=1)
def _windows_snapshot_api() -> SimpleNamespace:
    """Bind immutable Win32 snapshot types/functions once per guard process."""

    import ctypes
    from ctypes import wintypes

    class ProcessEntry32W(ctypes.Structure):
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

    class ProcessBasicInformation(ctypes.Structure):
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
    counters_type = _windows_process_memory_counters_type(ctypes, wintypes)
    create_snapshot = kernel32.CreateToolhelp32Snapshot
    create_snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
    create_snapshot.restype = wintypes.HANDLE
    process_first = kernel32.Process32FirstW
    process_first.argtypes = [wintypes.HANDLE, ctypes.POINTER(ProcessEntry32W)]
    process_first.restype = wintypes.BOOL
    process_next = kernel32.Process32NextW
    process_next.argtypes = [wintypes.HANDLE, ctypes.POINTER(ProcessEntry32W)]
    process_next.restype = wintypes.BOOL
    close_handle = kernel32.CloseHandle
    close_handle.argtypes = [wintypes.HANDLE]
    close_handle.restype = wintypes.BOOL
    open_process = kernel32.OpenProcess
    open_process.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    open_process.restype = wintypes.HANDLE
    get_process_times = kernel32.GetProcessTimes
    get_process_times.argtypes = [
        wintypes.HANDLE,
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
    ]
    get_process_times.restype = wintypes.BOOL
    query_image = kernel32.QueryFullProcessImageNameW
    query_image.argtypes = [
        wintypes.HANDLE,
        wintypes.DWORD,
        wintypes.LPWSTR,
        ctypes.POINTER(wintypes.DWORD),
    ]
    query_image.restype = wintypes.BOOL
    read_memory = kernel32.ReadProcessMemory
    read_memory.argtypes = [
        wintypes.HANDLE,
        wintypes.LPCVOID,
        wintypes.LPVOID,
        ctypes.c_size_t,
        ctypes.POINTER(ctypes.c_size_t),
    ]
    read_memory.restype = wintypes.BOOL
    query_basic = ntdll.NtQueryInformationProcess
    query_basic.argtypes = [
        wintypes.HANDLE,
        wintypes.ULONG,
        ctypes.c_void_p,
        wintypes.ULONG,
        ctypes.POINTER(wintypes.ULONG),
    ]
    query_basic.restype = wintypes.LONG
    pointer_size = ctypes.sizeof(ctypes.c_void_p)
    command_line_offset = 0x70 if pointer_size == 8 else 0x40
    return SimpleNamespace(
        ctypes=ctypes,
        wintypes=wintypes,
        ProcessEntry32W=ProcessEntry32W,
        ProcessBasicInformation=ProcessBasicInformation,
        counters_type=counters_type,
        create_snapshot=create_snapshot,
        process_first=process_first,
        process_next=process_next,
        close_handle=close_handle,
        open_process=open_process,
        get_process_memory_info=_windows_get_process_memory_info(
            psapi, ctypes, wintypes, counters_type
        ),
        get_process_times=get_process_times,
        query_image=query_image,
        read_memory=read_memory,
        query_basic=query_basic,
        pointer_size=pointer_size,
        peb_process_parameters_offset=0x20 if pointer_size == 8 else 0x10,
        command_line_offset=command_line_offset,
        command_line_buffer_offset=command_line_offset
        + (8 if pointer_size == 8 else 4),
        invalid_handle_value=wintypes.HANDLE(-1).value,
    )


def _snapshot_read_memory(api, handle, address, size, enforce_deadline):
    enforce_deadline("reading process memory")
    if address <= 0 or size <= 0:
        return None
    buffer = (api.ctypes.c_ubyte * size)()
    bytes_read = api.ctypes.c_size_t(0)
    if not api.read_memory(
        handle,
        api.ctypes.c_void_p(address),
        buffer,
        size,
        api.ctypes.byref(bytes_read),
    ):
        return None
    enforce_deadline("reading process memory")
    return None if bytes_read.value <= 0 else bytes(buffer[: bytes_read.value])


def _snapshot_read_integer(api, handle, address, size, enforce_deadline):
    raw = _snapshot_read_memory(api, handle, address, size, enforce_deadline)
    if raw is None or len(raw) != size:
        return None
    return int.from_bytes(raw, "little", signed=False)


def _snapshot_basic_info(api, handle):
    returned = api.wintypes.ULONG(0)
    info = api.ProcessBasicInformation()
    status = api.query_basic(
        handle,
        0,
        api.ctypes.byref(info),
        api.ctypes.sizeof(info),
        api.ctypes.byref(returned),
    )
    return None if status != 0 else info


def _snapshot_command_line(api, handle, enforce_deadline):
    enforce_deadline("reading process command line")
    info = _snapshot_basic_info(api, handle)
    if info is None or not info.PebBaseAddress:
        return None
    process_parameters = _snapshot_read_integer(
        api,
        handle,
        int(info.PebBaseAddress) + api.peb_process_parameters_offset,
        api.pointer_size,
        enforce_deadline,
    )
    if not process_parameters:
        return None
    byte_len = _snapshot_read_integer(
        api,
        handle,
        process_parameters + api.command_line_offset,
        2,
        enforce_deadline,
    )
    buffer_address = _snapshot_read_integer(
        api,
        handle,
        process_parameters + api.command_line_buffer_offset,
        api.pointer_size,
        enforce_deadline,
    )
    if not byte_len or not buffer_address:
        return None
    raw = _snapshot_read_memory(
        api,
        handle,
        buffer_address,
        min(byte_len, 32768),
        enforce_deadline,
    )
    if raw is None:
        return None
    enforce_deadline("reading process command line")
    return raw.decode("utf-16-le", errors="replace").strip("\x00")


def _snapshot_image_name(api, handle, enforce_deadline):
    enforce_deadline("reading process image name")
    size = api.wintypes.DWORD(32768)
    buffer = api.ctypes.create_unicode_buffer(size.value)
    if api.query_image(handle, 0, buffer, api.ctypes.byref(size)):
        enforce_deadline("reading process image name")
        return buffer.value
    return None


def _windows_process_snapshot_rows() -> list[
    tuple[int, int, int, str, int | None, int | None]
]:
    if os.name != "nt":
        return []
    timeout_sec = _windows_process_snapshot_timeout_sec()
    deadline = None if timeout_sec is None else time.monotonic() + timeout_sec

    def enforce_deadline(stage: str) -> None:
        if deadline is None or time.monotonic() <= deadline:
            return
        raise WindowsProcessSnapshotTimeout(
            f"Windows process snapshot exceeded {timeout_sec:.3f}s while {stage}"
        )

    api = _windows_snapshot_api()
    ctypes = api.ctypes
    wintypes = api.wintypes
    process_memory_counters_type = api.counters_type
    create_snapshot = api.create_snapshot
    process_first = api.process_first
    process_next = api.process_next
    close_handle = api.close_handle
    open_process = api.open_process
    get_process_memory_info = api.get_process_memory_info
    get_process_times = api.get_process_times
    TH32CS_SNAPPROCESS = 0x00000002
    PROCESS_QUERY_INFORMATION = 0x0400
    PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
    PROCESS_VM_READ = 0x0010

    enforce_deadline("creating process snapshot")
    snapshot = create_snapshot(TH32CS_SNAPPROCESS, 0)
    if snapshot == api.invalid_handle_value:
        return []
    rows: list[tuple[int, int, int, str, int | None, int | None]] = []
    try:
        entry = api.ProcessEntry32W()
        entry.dwSize = ctypes.sizeof(api.ProcessEntry32W)
        ok = process_first(snapshot, ctypes.byref(entry))
        now = time.time()
        while ok:
            enforce_deadline("enumerating process snapshot")
            pid = int(entry.th32ProcessID)
            if pid > 0:
                ppid = 0
                bound_pid: int | None = None
                bound_ppid: int | None = None
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
                    enforce_deadline("opening process")
                    handle = open_process(access, False, pid)
                    if handle:
                        break
                if handle:
                    try:
                        enforce_deadline("reading process metadata")
                        basic_info = _snapshot_basic_info(api, handle)
                        bound_pid = (
                            None
                            if basic_info is None
                            else int(basic_info.UniqueProcessId)
                        )
                        if basic_info is not None and bound_pid == pid:
                            bound_ppid = int(basic_info.InheritedFromUniqueProcessId)
                        image_name = _snapshot_image_name(
                            api,
                            handle,
                            enforce_deadline,
                        )
                        if _windows_process_needs_full_command_line(exe_name):
                            command = (
                                _snapshot_command_line(
                                    api,
                                    handle,
                                    enforce_deadline,
                                )
                                or image_name
                                or command
                            )
                        else:
                            command = image_name or command
                        counters = process_memory_counters_type()
                        counters.cb = ctypes.sizeof(process_memory_counters_type)
                        if get_process_memory_info(
                            handle,
                            ctypes.byref(counters),
                            counters.cb,
                        ):
                            enforce_deadline("reading process memory counters")
                            rss_kb = _working_set_rss_kb(counters)
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
                            enforce_deadline("reading process times")
                            started_at_ns = _filetime_to_unix_ns(
                                int(created.dwLowDateTime),
                                int(created.dwHighDateTime),
                            )
                            if started_at_ns is not None:
                                elapsed_sec = max(
                                    0,
                                    int(now - started_at_ns / 1_000_000_000),
                                )
                    finally:
                        close_handle(handle)
                ppid, started_at_ns = _validated_windows_process_binding(
                    pid,
                    bound_pid,
                    bound_ppid,
                    started_at_ns,
                )
                if started_at_ns is None:
                    elapsed_sec = None
                rows.append(
                    (
                        pid,
                        ppid,
                        rss_kb,
                        command,
                        elapsed_sec,
                        started_at_ns,
                    )
                )
            enforce_deadline("advancing process snapshot")
            ok = process_next(snapshot, ctypes.byref(entry))
    finally:
        close_handle(snapshot)
    return rows


def _main(argv: list[str]) -> int:
    if argv != [WINDOWS_PROCESS_SNAPSHOT_HELPER_ARG]:
        return 2
    rows = _windows_process_snapshot_rows()
    print(json.dumps(rows, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(_main(sys.argv[1:]))
