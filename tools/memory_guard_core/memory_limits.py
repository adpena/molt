from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
import os
from pathlib import Path
import subprocess
import sys


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


def _normalize_env_prefix(prefix: str | None) -> str:
    if not prefix:
        return ""
    return prefix.strip().upper().rstrip("_")


def _prefixed_names(prefix: str | None, suffixes: Sequence[str]) -> list[str]:
    normalized = _normalize_env_prefix(prefix)
    names: list[str] = []
    if normalized:
        names.extend(f"{normalized}_{suffix}" for suffix in suffixes)
    names.extend(f"MOLT_{suffix}" for suffix in suffixes)
    return list(dict.fromkeys(names))


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
        )
    except (OSError, subprocess.TimeoutExpired, TypeError):
        result = None
    if result is not None and result.returncode == 0:
        raw = result.stdout.strip()
        if raw.isdigit():
            return int(raw)
    return None


def _parse_darwin_vm_stat_available_bytes(text: str) -> int | None:
    page_size: int | None = None
    pages: dict[str, int] = {}
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        if line.startswith("Mach Virtual Memory Statistics:"):
            marker = "page size of "
            if marker in line:
                suffix = line.split(marker, 1)[1]
                digits = "".join(ch for ch in suffix if ch.isdigit())
                if digits:
                    page_size = int(digits)
            continue
        if ":" not in line:
            continue
        name, raw_value = line.split(":", 1)
        digits = "".join(ch for ch in raw_value if ch.isdigit())
        if digits:
            pages[name.strip().strip('"')] = int(digits)
    if page_size is None or page_size <= 0:
        return None
    available_pages = sum(
        pages.get(name, 0)
        for name in (
            "Pages free",
            "Pages inactive",
            "Pages speculative",
            "Pages purgeable",
        )
    )
    if available_pages <= 0:
        return None
    return available_pages * page_size


def _darwin_available_memory_bytes() -> int | None:
    try:
        result = subprocess.run(
            ["vm_stat"],
            capture_output=True,
            text=True,
            timeout=1.0,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired, TypeError):
        return None
    if result.returncode != 0:
        return None
    return _parse_darwin_vm_stat_available_bytes(result.stdout)


def physical_memory_bytes(
    prefix: str | None = None,
    environ: Mapping[str, str] | None = None,
) -> int | None:
    source = os.environ if environ is None else environ
    override = _float_env(
        source,
        _prefixed_names(prefix, ("TOTAL_MEMORY_GB", "MEMORY_TOTAL_GB")),
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
        _prefixed_names(prefix, ("MEM_AVAILABLE_GB", "MEMORY_AVAILABLE_GB")),
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
        _prefixed_names(prefix, ("MEMORY_RESERVE_GB", "MEM_RESERVE_GB")),
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
