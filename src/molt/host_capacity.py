"""How much work this machine can run (docs/design/host_capacity.md).

The one host capacity authority. A tool asks it how many workers of a
declared unit fit on this machine, never for a fixed number, so the same
command sizes itself on an 8 GB laptop, a 128 GB workstation and a CI runner.
"""

from __future__ import annotations

import os
import sys
import threading
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any, cast

_CGROUP_ROOT = Path("/sys/fs/cgroup")
_CGROUP_MEMBERSHIP = Path("/proc/self/cgroup")
# The memory a parent lets this process tree use: the memory guard exports
# the tree cap it enforces.
MEMORY_SLICE_ENV = "MOLT_HOST_MEMORY_SLICE_BYTES"
# Ad hoc or unowned pytest targets use this unit; registered commands and
# local cohorts project their measured owning envelopes from the proof plan.
DEFAULT_PYTEST_WORKER_MEMORY_GB = 2.5
_GIB = 1024**3
# What the operating system and its supervisors keep of physical memory: a
# fraction, at least the minimum and at most the maximum.
RESERVE_FRACTION = 0.06
RESERVE_MIN_BYTES = 1 * _GIB
RESERVE_MAX_BYTES = 12 * _GIB


def cpu_count(
    *,
    cgroup_root: Path = _CGROUP_ROOT,
    membership_path: Path = _CGROUP_MEMBERSHIP,
) -> int:
    """CPUs this process may run on.

    The rule of Rust's ``available_parallelism``, which the backend daemon
    already uses: the process's CPU affinity, then the Linux cgroup CPU quota
    (quota divided by period, rounded down, at least 1). ``os.cpu_count``
    alone counts every CPU of the machine, including ones the process may
    not use.
    """
    count: int | None
    if hasattr(os, "process_cpu_count"):  # Python 3.13+: affinity everywhere
        count = os.process_cpu_count()
    elif hasattr(os, "sched_getaffinity"):
        count = len(os.sched_getaffinity(0))
    else:
        count = os.cpu_count()
    count = max(1, count or 1)
    if sys.platform.startswith("linux"):
        quota = cgroup_cpu_quota(
            cgroup_root=cgroup_root, membership_path=membership_path
        )
        if quota is not None:
            count = max(1, min(count, int(quota)))
    return count


def cgroup_cpu_quota(
    *,
    cgroup_root: Path = _CGROUP_ROOT,
    membership_path: Path = _CGROUP_MEMBERSHIP,
) -> float | None:
    """The tightest CPU quota on this process's cgroup or its ancestors, in
    CPUs, or None when none is set (cgroup v2 ``cpu.max``, v1 CFS files)."""
    try:
        memberships = membership_path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return None
    limits: list[float] = []
    for line in memberships:
        _hierarchy, separator, rest = line.partition(":")
        controllers, separator2, relative = rest.partition(":")
        if not separator or not separator2:
            continue
        if not controllers:
            base, reader = cgroup_root, _unified_quota
        elif "cpu" in controllers.split(","):
            base = cgroup_root / controllers
            if not base.is_dir():
                base = cgroup_root / "cpu"
            reader = _legacy_quota
        else:
            continue
        directory = base / relative.lstrip("/")
        # A quota on any ancestor bounds every cgroup beneath it.
        for candidate in (directory, *directory.parents):
            if candidate != base and base not in candidate.parents:
                break
            limit = reader(candidate)
            if limit is not None:
                limits.append(limit)
    return min(limits) if limits else None


def _unified_quota(directory: Path) -> float | None:
    try:
        quota, _, period = (
            (directory / "cpu.max").read_text(encoding="utf-8").partition(" ")
        )
    except OSError:
        return None
    return _ratio(quota.strip(), period.strip())


def _legacy_quota(directory: Path) -> float | None:
    try:
        quota = (directory / "cpu.cfs_quota_us").read_text(encoding="utf-8").strip()
        period = (directory / "cpu.cfs_period_us").read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return _ratio(quota, period)


def _ratio(quota: str, period: str) -> float | None:
    # "max" (v2) or -1 (v1) is no quota.
    if not quota.isdigit() or not period.isdigit() or int(period) == 0:
        return None
    return int(quota) / int(period)


# --------------------------------------------------------------------------
# Memory
# --------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class MemoryCapacity:
    """One host observation and its separately delegated allocation ceiling."""

    total_bytes: int | None
    available_bytes: int | None
    tree_limit_bytes: int | None = None

    @property
    def bounded_bytes(self) -> tuple[int | None, int | None]:
        """Capacity within the allocation, for worker scheduling."""
        limit = self.tree_limit_bytes
        if limit is None:
            return self.total_bytes, self.available_bytes
        return (
            limit if self.total_bytes is None else min(self.total_bytes, limit),
            limit if self.available_bytes is None else min(self.available_bytes, limit),
        )


def memory_capacity(environ: Mapping[str, str] | None = None) -> MemoryCapacity:
    """Raw total/live host memory and the parent's allocation, one snapshot.

    Linux bounds both by the active cgroup; macOS reads the Mach VM counters
    vm_stat prints; Windows asks GlobalMemoryStatusEx. Keep the exported tree
    limit separate: a nested guard must not reserve host memory again from it.
    """
    total, available = _probe_memory_bytes()
    source = os.environ if environ is None else environ
    raw = source.get(MEMORY_SLICE_ENV, "").strip()
    if not raw:
        return MemoryCapacity(total, available)
    if not raw.isdigit() or int(raw) <= 0:
        raise ValueError(f"{MEMORY_SLICE_ENV} must be a positive integer, got {raw!r}")
    return MemoryCapacity(total, available, int(raw))


def _probe_memory_bytes() -> tuple[int | None, int | None]:
    if os.name == "nt":
        return _windows_memory_bytes()
    if sys.platform.startswith("linux"):
        total, available = linux_memory_bytes()
        if total is not None or available is not None:
            return total, available
    if sys.platform == "darwin":
        parsed = darwin_vm_pages()
        available = None if parsed is None else darwin_available_bytes(*parsed)
        return _darwin_sysctl_integer("hw.memsize"), available
    try:
        page_size = os.sysconf("SC_PAGE_SIZE")
        total_pages = os.sysconf("SC_PHYS_PAGES")
        available_pages = os.sysconf("SC_AVPHYS_PAGES")
    except (ValueError, OSError, AttributeError):
        return None, None
    if page_size <= 0:
        return None, None
    total = int(page_size) * int(total_pages) if total_pages > 0 else None
    available = int(page_size) * int(available_pages) if available_pages > 0 else None
    return total, available


def headroom_bytes(capacity: MemoryCapacity) -> int:
    """Memory a unit of work must leave free in this process's context.

    The machine's reserve, unless a parent exported a memory slice: every
    slice (a guard's tree cap, an executor's per-command share) is cut from
    memory beyond that reserve, so it already holds it.
    """
    if capacity.tree_limit_bytes is not None:
        return 0
    return reserve_bytes(capacity.total_bytes)


def reserve_bytes(total_bytes: int | None) -> int:
    """Memory no unit of work may claim on a machine with ``total_bytes``."""
    if total_bytes is None:
        return RESERVE_MIN_BYTES
    return int(
        min(RESERVE_MAX_BYTES, max(RESERVE_MIN_BYTES, total_bytes * RESERVE_FRACTION))
    )


def worker_count(
    *,
    bytes_per_worker: int,
    headroom_bytes: int,
    cpus: int | None = None,
) -> int:
    """Workers of ``bytes_per_worker`` that fit beside ``headroom_bytes``,
    bounded by this process's CPUs and live memory."""
    if bytes_per_worker <= 0 or headroom_bytes < 0:
        raise ValueError("worker memory policy must be positive")
    total_memory_bytes, available_memory_bytes = memory_capacity().bounded_bytes
    return worker_count_from_samples(
        bytes_per_worker=bytes_per_worker,
        headroom_bytes=headroom_bytes,
        total_memory_bytes=total_memory_bytes,
        available_memory_bytes=available_memory_bytes,
        cpus=cpus,
    )


def worker_count_from_samples(
    *,
    bytes_per_worker: int,
    headroom_bytes: int,
    total_memory_bytes: int | None,
    available_memory_bytes: int | None,
    cpus: int | None = None,
) -> int:
    """Compute a worker ceiling from one coherent resource snapshot."""

    if bytes_per_worker <= 0 or headroom_bytes < 0:
        raise ValueError("worker memory policy must be positive")
    cpus = max(1, cpus if cpus is not None else cpu_count())
    memory_samples = [
        sample
        for sample in (total_memory_bytes, available_memory_bytes)
        if sample is not None
    ]
    if not memory_samples:
        return cpus
    usable = max(0, min(memory_samples) - headroom_bytes)
    memory_workers = max(1, usable // bytes_per_worker)
    return max(1, min(cpus, memory_workers))


def _windows_memory_bytes() -> tuple[int | None, int | None]:
    if os.name != "nt":
        return None, None
    import ctypes

    class _MemoryStatusEx(ctypes.Structure):
        _fields_ = [
            ("dwLength", ctypes.c_ulong),
            ("dwMemoryLoad", ctypes.c_ulong),
            ("ullTotalPhys", ctypes.c_ulonglong),
            ("ullAvailPhys", ctypes.c_ulonglong),
            ("ullTotalPageFile", ctypes.c_ulonglong),
            ("ullAvailPageFile", ctypes.c_ulonglong),
            ("ullTotalVirtual", ctypes.c_ulonglong),
            ("ullAvailVirtual", ctypes.c_ulonglong),
            ("ullAvailExtendedVirtual", ctypes.c_ulonglong),
        ]

    status = _MemoryStatusEx()
    status.dwLength = ctypes.sizeof(_MemoryStatusEx)
    try:
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.GlobalMemoryStatusEx.restype = ctypes.c_int
        kernel32.GlobalMemoryStatusEx.argtypes = [ctypes.POINTER(_MemoryStatusEx)]
        if kernel32.GlobalMemoryStatusEx(ctypes.byref(status)):
            return int(status.ullTotalPhys), int(status.ullAvailPhys)
    except (OSError, AttributeError, ValueError):
        pass
    return None, None


def _read_memory_integer(path: Path, *, allow_zero: bool = False) -> int | None:
    try:
        raw = path.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    if not raw.isdigit():
        return None
    value = int(raw)
    return value if value > 0 or (allow_zero and value == 0) else None


def _darwin_sysctl_integer(name: str) -> int | None:
    """Read an integer sysctl without spawning the ``sysctl`` executable."""

    if sys.platform != "darwin":
        return None
    import ctypes

    try:
        libc = ctypes.CDLL(None, use_errno=True)
        sysctlbyname = libc.sysctlbyname
        sysctlbyname.restype = ctypes.c_int
        sysctlbyname.argtypes = [
            ctypes.c_char_p,
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_size_t),
            ctypes.c_void_p,
            ctypes.c_size_t,
        ]
        encoded = name.encode("ascii")
        size = ctypes.c_size_t()
        if sysctlbyname(encoded, None, ctypes.byref(size), None, 0) != 0:
            return None
        if size.value <= 0 or size.value > ctypes.sizeof(ctypes.c_uint64):
            return None
        storage = ctypes.create_string_buffer(size.value)
        if sysctlbyname(encoded, storage, ctypes.byref(size), None, 0) != 0:
            return None
    except (AttributeError, OSError, ValueError):
        return None
    return int.from_bytes(storage.raw[: size.value], byteorder=sys.byteorder)


def _read_cgroup_inactive_file_bytes(path: Path) -> int:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return 0
    for line in lines:
        name, separator, raw = line.partition(" ")
        if separator and name in {"inactive_file", "total_inactive_file"}:
            return int(raw) if raw.isdigit() else 0
    return 0


def _linux_cgroup_memory_directories(
    *,
    cgroup_root: Path,
    membership_path: Path,
) -> tuple[Path, Path]:
    unified_relative: Path | None = None
    legacy_relative: Path | None = None
    try:
        memberships = membership_path.read_text(encoding="utf-8").splitlines()
    except OSError:
        memberships = []
    for line in memberships:
        _hierarchy, separator, suffix = line.partition(":")
        controllers, separator2, relative = suffix.partition(":")
        if not separator or not separator2:
            continue
        candidate = Path(relative.lstrip("/"))
        if not controllers:
            unified_relative = candidate
        elif "memory" in controllers.split(","):
            legacy_relative = candidate
    unified = cgroup_root / (unified_relative or Path())
    if not (unified / "memory.max").exists():
        unified = cgroup_root
    legacy_base = cgroup_root / "memory"
    legacy = legacy_base / (legacy_relative or Path())
    if not (legacy / "memory.limit_in_bytes").exists():
        legacy = legacy_base
    return unified, legacy


def linux_memory_bytes(
    *,
    meminfo_path: Path = Path("/proc/meminfo"),
    cgroup_root: Path = Path("/sys/fs/cgroup"),
    cgroup_membership_path: Path = Path("/proc/self/cgroup"),
) -> tuple[int | None, int | None]:
    """Sample Linux host memory constrained by the active cgroup, if any."""

    fields: dict[str, int] = {}
    try:
        meminfo = meminfo_path.read_text(encoding="utf-8")
    except OSError:
        meminfo = ""
    for line in meminfo.splitlines():
        name, separator, value = line.partition(":")
        if not separator or name not in {"MemTotal", "MemAvailable"}:
            continue
        parts = value.split()
        if parts and parts[0].isdigit():
            fields[name] = int(parts[0]) * 1024

    total = fields.get("MemTotal")
    available = fields.get("MemAvailable")
    unified, legacy = _linux_cgroup_memory_directories(
        cgroup_root=cgroup_root,
        membership_path=cgroup_membership_path,
    )
    cgroup_limit = _read_memory_integer(unified / "memory.max")
    cgroup_usage = _read_memory_integer(unified / "memory.current", allow_zero=True)
    cgroup_inactive_file = _read_cgroup_inactive_file_bytes(unified / "memory.stat")
    if cgroup_limit is None:
        cgroup_limit = _read_memory_integer(legacy / "memory.limit_in_bytes")
        cgroup_usage = _read_memory_integer(
            legacy / "memory.usage_in_bytes", allow_zero=True
        )
        cgroup_inactive_file = _read_cgroup_inactive_file_bytes(legacy / "memory.stat")
    if cgroup_limit is not None:
        total = cgroup_limit if total is None else min(total, cgroup_limit)
        if cgroup_usage is not None:
            reclaimable = min(cgroup_usage, cgroup_inactive_file)
            cgroup_available = max(0, cgroup_limit - cgroup_usage + reclaimable)
            available = (
                cgroup_available
                if available is None
                else min(available, cgroup_available)
            )
    return total, available


# The vm_stat rows counted as available memory.
DARWIN_AVAILABLE_PAGE_ROWS = (
    "Pages free",
    "Pages inactive",
    "Pages speculative",
    "Pages purgeable",
)
# <mach/host_info.h> HOST_VM_INFO64 and its count of natural_t words.
_DARWIN_HOST_VM_INFO64 = 4
_DARWIN_VM_STATISTICS64_SIZE = 152


@dataclass(frozen=True, slots=True)
class _DarwinHostVm:
    """One process-wide binding of the Mach host VM statistics call."""

    ctypes: Any
    host: int
    statistics_type: type[Any]
    host_statistics64: Callable[..., int]
    page_size: int


def _load_darwin_host_vm() -> _DarwinHostVm:
    import ctypes

    class VmStatistics64(ctypes.Structure):
        # <mach/vm_statistics.h> struct vm_statistics64, natural alignment.
        _fields_ = [
            ("free_count", ctypes.c_uint32),
            ("active_count", ctypes.c_uint32),
            ("inactive_count", ctypes.c_uint32),
            ("wire_count", ctypes.c_uint32),
            ("zero_fill_count", ctypes.c_uint64),
            ("reactivations", ctypes.c_uint64),
            ("pageins", ctypes.c_uint64),
            ("pageouts", ctypes.c_uint64),
            ("faults", ctypes.c_uint64),
            ("cow_faults", ctypes.c_uint64),
            ("lookups", ctypes.c_uint64),
            ("hits", ctypes.c_uint64),
            ("purges", ctypes.c_uint64),
            ("purgeable_count", ctypes.c_uint32),
            ("speculative_count", ctypes.c_uint32),
            ("decompressions", ctypes.c_uint64),
            ("compressions", ctypes.c_uint64),
            ("swapins", ctypes.c_uint64),
            ("swapouts", ctypes.c_uint64),
            ("compressor_page_count", ctypes.c_uint32),
            ("throttled_count", ctypes.c_uint32),
            ("external_page_count", ctypes.c_uint32),
            ("internal_page_count", ctypes.c_uint32),
            ("total_uncompressed_pages_in_compressor", ctypes.c_uint64),
        ]

    if ctypes.sizeof(VmStatistics64) != _DARWIN_VM_STATISTICS64_SIZE:
        raise OSError(
            f"vm_statistics64 layout is {ctypes.sizeof(VmStatistics64)} bytes, "
            f"kernel ABI needs {_DARWIN_VM_STATISTICS64_SIZE}"
        )
    libsystem = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
    mach_host_self = libsystem.mach_host_self
    mach_host_self.argtypes = []
    mach_host_self.restype = ctypes.c_uint32
    host_page_size = libsystem.host_page_size
    host_page_size.argtypes = [ctypes.c_uint32, ctypes.POINTER(ctypes.c_size_t)]
    host_page_size.restype = ctypes.c_int
    host_statistics64 = libsystem.host_statistics64
    host_statistics64.argtypes = [
        ctypes.c_uint32,
        ctypes.c_int,
        ctypes.POINTER(VmStatistics64),
        ctypes.POINTER(ctypes.c_uint32),
    ]
    host_statistics64.restype = ctypes.c_int
    # One send right for the life of the process: every mach_host_self call
    # adds a reference to the same port.
    host = int(mach_host_self())
    page_size = ctypes.c_size_t(0)
    if host_page_size(host, ctypes.byref(page_size)) != 0 or page_size.value <= 0:
        raise OSError("host_page_size failed")
    return _DarwinHostVm(
        ctypes=ctypes,
        host=host,
        statistics_type=VmStatistics64,
        host_statistics64=host_statistics64,
        page_size=int(page_size.value),
    )


_DARWIN_HOST_VM_UNSET = object()
_darwin_host_vm_cache: _DarwinHostVm | None | object = _DARWIN_HOST_VM_UNSET
_darwin_host_vm_lock = threading.Lock()


def _darwin_host_vm() -> _DarwinHostVm | None:
    """Return the one cached Mach binding, including cached unavailability."""

    global _darwin_host_vm_cache
    cached = _darwin_host_vm_cache
    if cached is _DARWIN_HOST_VM_UNSET:
        with _darwin_host_vm_lock:
            cached = _darwin_host_vm_cache
            if cached is _DARWIN_HOST_VM_UNSET:
                try:
                    cached = _load_darwin_host_vm()
                except (AttributeError, OSError, TypeError, ValueError):
                    cached = None
                _darwin_host_vm_cache = cached
    return None if cached is None else cast(_DarwinHostVm, cached)


def darwin_vm_pages() -> tuple[int, dict[str, int]] | None:
    """Return the kernel page size and vm_stat's page counts by row name.

    One ``host_statistics64(HOST_VM_INFO64)`` call reads the counters vm_stat
    prints, derived the way vm_stat derives them (its "Pages free" excludes
    the speculative pages the kernel counts as free), without a subprocess.
    The kernel rate-limits this call for non-platform binaries: in each
    one-second window a process gets 2-10 fresh reads, then the cached
    counters, so a caller sees memory at most about a second old.
    """

    binding = _darwin_host_vm()
    if binding is None:
        return None
    ctypes = binding.ctypes
    stats = binding.statistics_type()
    count = ctypes.c_uint32(_DARWIN_VM_STATISTICS64_SIZE // 4)
    if (
        binding.host_statistics64(
            binding.host,
            _DARWIN_HOST_VM_INFO64,
            ctypes.byref(stats),
            ctypes.byref(count),
        )
        != 0
    ):
        return None
    pages = {
        "Pages free": max(0, int(stats.free_count) - int(stats.speculative_count)),
        "Pages active": int(stats.active_count),
        "Pages inactive": int(stats.inactive_count),
        "Pages speculative": int(stats.speculative_count),
        "Pages throttled": int(stats.throttled_count),
        "Pages wired down": int(stats.wire_count),
        "Pages purgeable": int(stats.purgeable_count),
        "File-backed pages": int(stats.external_page_count),
        "Anonymous pages": int(stats.internal_page_count),
        "Pages stored in compressor": int(stats.total_uncompressed_pages_in_compressor),
        "Pages occupied by compressor": int(stats.compressor_page_count),
    }
    return binding.page_size, pages


def darwin_available_bytes(page_size: int, pages: Mapping[str, int]) -> int | None:
    """The bytes counted as available from vm_stat's page rows."""

    available_pages = sum(pages.get(name, 0) for name in DARWIN_AVAILABLE_PAGE_ROWS)
    if page_size <= 0 or available_pages <= 0:
        return None
    return available_pages * page_size
