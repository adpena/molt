from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
import os
from pathlib import Path
import subprocess
import sys
import threading
from typing import Any, cast


DEFAULT_MAX_RSS_GB = 12.0
DEFAULT_MAX_TOTAL_RSS_GB = 18.0
DEFAULT_MAX_GLOBAL_RSS_GB = 36.0
DEFAULT_HARD_MAX_RSS_GB = 112.0
DEFAULT_HARD_MAX_GLOBAL_RSS_GB = 4096.0
DEFAULT_HARD_MAX_CHILD_RLIMIT_GB = 4096.0
DEFAULT_MEMORY_RESERVE_FRACTION = 0.06
DEFAULT_MEMORY_RESERVE_MIN_GB = 1.0
DEFAULT_MEMORY_RESERVE_MAX_GB = 12.0
DEFAULT_GLOBAL_FRACTION_OF_USABLE = 0.97
DEFAULT_TOTAL_FRACTION_OF_GLOBAL = 0.60
DEFAULT_PROCESS_FRACTION_OF_TOTAL = 0.90
_RSS_HARD_MARGIN_GB = 0.001


@dataclass(frozen=True, slots=True)
class AdaptiveMemoryBudget:
    max_process_rss_gb: float
    max_total_rss_gb: float
    max_global_rss_gb: float
    reserve_gb: float
    physical_gb: float | None
    available_gb: float | None
    source: str
    accounted_rss_gb: float = 0.0


@dataclass(frozen=True, slots=True)
class ResolvedMemoryLimits:
    max_process_rss_kb: int
    max_total_rss_kb: int | None
    max_global_rss_kb: int | None = None
    adaptive_budget: AdaptiveMemoryBudget | None = None
    dynamic_process_rss: bool = False
    dynamic_total_rss: bool = False
    dynamic_global_rss: bool = False

    @property
    def max_process_rss_gb(self) -> float:
        return self.max_process_rss_kb / (1024 * 1024)

    @property
    def max_total_rss_gb(self) -> float | None:
        if self.max_total_rss_kb is None:
            return None
        return self.max_total_rss_kb / (1024 * 1024)

    @property
    def max_global_rss_gb(self) -> float | None:
        if self.max_global_rss_kb is None:
            return None
        return self.max_global_rss_kb / (1024 * 1024)


def normalize_env_prefix(prefix: str | None) -> str:
    """The guard scope stem for ``prefix``: upper-cased, no trailing underscore.

    One authority for every ``<STEM>_<SUFFIX>`` environment name the guards
    compose (see ``[[family]]`` in ``src/molt/environment_registry.toml``).
    """

    if not prefix:
        return ""
    return prefix.strip().upper().rstrip("_")


def _prefixed_names(prefix: str | None, suffix: str) -> list[str]:
    """``<STEM>_<suffix>`` then the root fallback ``MOLT_<suffix>``."""

    normalized = normalize_env_prefix(prefix)
    names: list[str] = []
    if normalized and normalized != "MOLT":
        names.append(f"{normalized}_{suffix}")
    names.append(f"MOLT_{suffix}")
    return names


def _float_env(environ: Mapping[str, str], names: Sequence[str]) -> float | None:
    for name in names:
        raw = environ.get(name)
        if raw is None or not raw.strip():
            continue
        try:
            value = float(raw)
        except ValueError:
            continue
        if value > 0:
            return value
    return None


def _below_hard_memory_cap(value_gb: float, hard_gb: float) -> float:
    return min(value_gb, hard_gb - _RSS_HARD_MARGIN_GB)


def _gb_from_bytes(value: int | None) -> float | None:
    if value is None or value <= 0:
        return None
    return value / (1024 * 1024 * 1024)


def _linux_meminfo_bytes(key: str) -> int | None:
    try:
        text = Path("/proc/meminfo").read_text(encoding="utf-8")
    except OSError:
        return None
    for line in text.splitlines():
        if not line.startswith(f"{key}:"):
            continue
        parts = line.split()
        if len(parts) >= 2 and parts[1].isdigit():
            return int(parts[1]) * 1024
    return None


def _darwin_physical_memory_bytes() -> int | None:
    try:
        return int(os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES"))
    except (OSError, ValueError, AttributeError):
        pass
    try:
        result = subprocess.run(
            ["sysctl", "-n", "hw.memsize"],
            capture_output=True,
            text=True,
            timeout=1.0,
            check=False,
            encoding="utf-8",
        )
    except (OSError, subprocess.TimeoutExpired, TypeError):
        result = None
    if result is not None and result.returncode == 0:
        raw = result.stdout.strip()
        if raw.isdigit():
            return int(raw)
    return None


# The vm_stat rows the guard counts as available memory.
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
    """The bytes the guard counts as available from vm_stat's page rows."""

    available_pages = sum(pages.get(name, 0) for name in DARWIN_AVAILABLE_PAGE_ROWS)
    if page_size <= 0 or available_pages <= 0:
        return None
    return available_pages * page_size


def _darwin_available_memory_bytes() -> int | None:
    parsed = darwin_vm_pages()
    return None if parsed is None else darwin_available_bytes(*parsed)


def physical_memory_bytes(
    prefix: str | None = None,
    environ: Mapping[str, str] | None = None,
) -> int | None:
    source = os.environ if environ is None else environ
    override = _float_env(
        source,
        _prefixed_names(prefix, "MEMORY_TOTAL_GB"),
    )
    if override is not None:
        return int(override * 1024 * 1024 * 1024)
    if sys.platform.startswith("linux"):
        return _linux_meminfo_bytes("MemTotal")
    if sys.platform == "darwin":
        return _darwin_physical_memory_bytes()
    try:
        return int(os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES"))
    except (OSError, ValueError, AttributeError):
        return None


def available_memory_bytes(
    prefix: str | None = None,
    environ: Mapping[str, str] | None = None,
) -> int | None:
    source = os.environ if environ is None else environ
    override = _float_env(
        source,
        _prefixed_names(prefix, "MEMORY_AVAILABLE_GB"),
    )
    if override is not None:
        return int(override * 1024 * 1024 * 1024)
    if sys.platform.startswith("linux"):
        return _linux_meminfo_bytes("MemAvailable")
    if sys.platform == "darwin":
        return _darwin_available_memory_bytes()
    return None


def adaptive_memory_budget(
    prefix: str | None = None,
    environ: Mapping[str, str] | None = None,
    *,
    accounted_rss_kb: int = 0,
) -> AdaptiveMemoryBudget:
    source = os.environ if environ is None else environ
    physical_gb = _gb_from_bytes(physical_memory_bytes(prefix, source))
    available_gb = _gb_from_bytes(available_memory_bytes(prefix, source))
    accounted_rss_gb = max(0, accounted_rss_kb) / (1024 * 1024)
    if available_gb is not None and accounted_rss_gb > 0:
        available_gb += accounted_rss_gb
    if physical_gb is not None and available_gb is not None:
        available_gb = min(available_gb, physical_gb)
    reserve_override = _float_env(
        source,
        _prefixed_names(prefix, "MEMORY_RESERVE_GB"),
    )
    if reserve_override is not None:
        reserve_gb = reserve_override
    elif physical_gb is not None:
        reserve_gb = min(
            DEFAULT_MEMORY_RESERVE_MAX_GB,
            max(
                DEFAULT_MEMORY_RESERVE_MIN_GB,
                physical_gb * DEFAULT_MEMORY_RESERVE_FRACTION,
            ),
        )
    else:
        reserve_gb = DEFAULT_MEMORY_RESERVE_MIN_GB

    if available_gb is not None:
        usable_gb = available_gb - reserve_gb
        if usable_gb <= 0:
            usable_gb = max(0.25, available_gb * 0.50)
        source_name = "available"
    elif physical_gb is not None:
        usable_gb = physical_gb * 0.75
        source_name = "physical"
    else:
        return AdaptiveMemoryBudget(
            max_process_rss_gb=DEFAULT_MAX_RSS_GB,
            max_total_rss_gb=DEFAULT_MAX_TOTAL_RSS_GB,
            max_global_rss_gb=DEFAULT_MAX_GLOBAL_RSS_GB,
            reserve_gb=reserve_gb,
            physical_gb=None,
            available_gb=None,
            source="fallback",
            accounted_rss_gb=accounted_rss_gb,
        )

    global_gb = max(0.25, usable_gb * DEFAULT_GLOBAL_FRACTION_OF_USABLE)
    if physical_gb is not None:
        global_gb = min(global_gb, max(0.25, physical_gb - reserve_gb))
    global_gb = _below_hard_memory_cap(
        global_gb,
        DEFAULT_HARD_MAX_GLOBAL_RSS_GB,
    )
    total_gb = min(
        global_gb,
        max(0.25, global_gb * DEFAULT_TOTAL_FRACTION_OF_GLOBAL),
    )
    total_gb = _below_hard_memory_cap(total_gb, DEFAULT_HARD_MAX_RSS_GB)
    process_gb = min(
        total_gb,
        max(0.25, total_gb * DEFAULT_PROCESS_FRACTION_OF_TOTAL),
    )
    process_gb = _below_hard_memory_cap(process_gb, DEFAULT_HARD_MAX_RSS_GB)
    return AdaptiveMemoryBudget(
        max_process_rss_gb=process_gb,
        max_total_rss_gb=total_gb,
        max_global_rss_gb=global_gb,
        reserve_gb=reserve_gb,
        physical_gb=physical_gb,
        available_gb=available_gb,
        source=source_name,
        accounted_rss_gb=accounted_rss_gb,
    )


def max_rss_kb_from_gb(value: float) -> int:
    if value <= 0:
        raise ValueError("max RSS must be greater than 0 GB")
    if value >= DEFAULT_HARD_MAX_RSS_GB:
        raise ValueError(f"max RSS must stay below {DEFAULT_HARD_MAX_RSS_GB:g} GB")
    return int(value * 1024 * 1024)


def max_global_rss_kb_from_gb(value: float) -> int:
    if value <= 0:
        raise ValueError("global RSS must be greater than 0 GB")
    if value >= DEFAULT_HARD_MAX_GLOBAL_RSS_GB:
        raise ValueError(
            f"global RSS must stay below {DEFAULT_HARD_MAX_GLOBAL_RSS_GB:g} GB"
        )
    return int(value * 1024 * 1024)


def child_rlimit_kb_from_gb(value: float) -> int:
    if value <= 0:
        raise ValueError("child resource limit must be greater than 0 GB")
    if value >= DEFAULT_HARD_MAX_CHILD_RLIMIT_GB:
        raise ValueError(
            "child resource limit must stay below "
            f"{DEFAULT_HARD_MAX_CHILD_RLIMIT_GB:g} GB"
        )
    return int(value * 1024 * 1024)


def default_child_rlimit_gb(
    *,
    max_process_rss_gb: float,
    max_total_rss_gb: float,
    max_global_rss_gb: float | None = None,
) -> float:
    limit_gb = min(DEFAULT_HARD_MAX_CHILD_RLIMIT_GB - 0.001, max_process_rss_gb)
    limit_gb = min(limit_gb, max_total_rss_gb)
    if max_global_rss_gb is not None:
        limit_gb = min(limit_gb, max_global_rss_gb)
    return limit_gb


def resolve_memory_limits(
    *,
    max_process_rss_kb: int,
    max_total_rss_kb: int | None = None,
    max_global_rss_kb: int | None = None,
    adaptive_budget_provider: Callable[[int], AdaptiveMemoryBudget] | None = None,
    dynamic_process_rss: bool = False,
    dynamic_total_rss: bool = False,
    dynamic_global_rss: bool = False,
    accounted_rss_kb: int = 0,
) -> ResolvedMemoryLimits:
    budget = None
    if adaptive_budget_provider is not None and (
        dynamic_process_rss or dynamic_total_rss or dynamic_global_rss
    ):
        budget = adaptive_budget_provider(max(0, accounted_rss_kb))
    process_kb = max_process_rss_kb
    total_kb = max_total_rss_kb
    global_kb = max_global_rss_kb
    if budget is not None:
        if dynamic_process_rss:
            process_kb = max_rss_kb_from_gb(budget.max_process_rss_gb)
        if dynamic_total_rss:
            total_kb = max_rss_kb_from_gb(budget.max_total_rss_gb)
        if dynamic_global_rss:
            global_kb = max_global_rss_kb_from_gb(budget.max_global_rss_gb)
    return ResolvedMemoryLimits(
        max_process_rss_kb=process_kb,
        max_total_rss_kb=total_kb,
        max_global_rss_kb=global_kb,
        adaptive_budget=budget,
        dynamic_process_rss=dynamic_process_rss,
        dynamic_total_rss=dynamic_total_rss,
        dynamic_global_rss=dynamic_global_rss,
    )
