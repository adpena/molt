#!/usr/bin/env python3
from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
import os

try:
    from tools import memory_guard
except ModuleNotFoundError:  # pragma: no cover - direct script import from tools/
    import memory_guard  # type: ignore


SCHEMA_VERSION = "molt.resource_pressure.v1"
DEFAULT_MAX_CARGO_BUILD_JOBS = 4
DEFAULT_CARGO_BUILD_GB_PER_JOB = 12.0
DEFAULT_MAX_COMPILE_SLOTS = 2
DEFAULT_COMPILE_GB_PER_SLOT = 12.0
DEFAULT_COMPILE_ACTIVE_PROCS_FACTOR = 3
DEFAULT_COMPILE_LOAD_PER_CPU = 1.25


@dataclass(frozen=True, slots=True)
class ResourcePressurePlan:
    prefix: str
    cpu_count: int
    pressure_level: str
    memory_source: str
    physical_gb: float | None
    available_gb: float | None
    reserve_gb: float
    usable_gb: float | None
    cargo_build_jobs: int
    cargo_build_gb_per_job: float
    max_cargo_build_jobs: int
    compile_max_slots: int
    compile_gb_per_slot: float
    compile_max_active_procs: int
    compile_max_load: float
    diff_scheduler_per_job_gb: float
    diff_max_jobs: int
    diff_global_gb: float
    diff_tree_gb: float
    reason: str

    def to_json_dict(self) -> dict[str, object]:
        return {
            "schema": SCHEMA_VERSION,
            "prefix": self.prefix,
            "cpu_count": self.cpu_count,
            "pressure": self.pressure_level,
            "memory": {
                "source": self.memory_source,
                "physical_gb": self.physical_gb,
                "available_gb": self.available_gb,
                "reserve_gb": self.reserve_gb,
                "usable_gb": self.usable_gb,
            },
            "cargo": {
                "build_jobs": self.cargo_build_jobs,
                "gb_per_job": self.cargo_build_gb_per_job,
                "max_jobs": self.max_cargo_build_jobs,
            },
            "compile": {
                "max_slots": self.compile_max_slots,
                "gb_per_slot": self.compile_gb_per_slot,
                "max_active_procs": self.compile_max_active_procs,
                "max_load": self.compile_max_load,
            },
            "diff": {
                "scheduler_per_job_gb": self.diff_scheduler_per_job_gb,
                "max_jobs": self.diff_max_jobs,
                "global_gb": self.diff_global_gb,
                "tree_gb": self.diff_tree_gb,
            },
            "reason": self.reason,
        }


def _usable_memory_gb(budget: memory_guard.AdaptiveMemoryBudget) -> float | None:
    measured_gb = budget.available_gb
    if measured_gb is None:
        measured_gb = budget.physical_gb
    if measured_gb is None:
        return None
    usable_gb = max(0.0, measured_gb - budget.reserve_gb)
    if usable_gb <= 0:
        usable_gb = max(0.25, measured_gb * 0.50)
    return usable_gb


def _memory_job_count(
    usable_gb: float | None,
    *,
    gb_per_job: float,
    fallback: int,
) -> int:
    if usable_gb is None:
        return max(1, fallback)
    return max(1, int(usable_gb // max(0.001, gb_per_job)))


def _pressure_level(budget: memory_guard.AdaptiveMemoryBudget) -> str:
    if budget.available_gb is None or budget.physical_gb is None:
        return "fallback"
    if budget.available_gb <= budget.reserve_gb:
        return "critical"
    ratio = budget.available_gb / max(0.001, budget.physical_gb)
    if ratio < 0.25:
        return "high"
    if ratio < 0.50:
        return "medium"
    return "low"


def scheduler_per_job_gb(
    *,
    global_gb: float,
    tree_gb: float,
    cpu_count: int,
    explicit_gb: float | None = None,
) -> float:
    if explicit_gb is not None and explicit_gb > 0:
        return max(0.001, min(explicit_gb, tree_gb))
    cpus = max(1, int(cpu_count))
    per_cpu_global_gb = global_gb / cpus
    floor_gb = min(tree_gb, 1.0)
    return max(
        0.001,
        min(tree_gb, max(floor_gb, min(8.0, per_cpu_global_gb))),
    )


def scheduler_max_jobs(
    *,
    global_gb: float,
    tree_gb: float,
    cpu_count: int,
    explicit_per_job_gb: float | None = None,
) -> int:
    per_job = scheduler_per_job_gb(
        global_gb=global_gb,
        tree_gb=tree_gb,
        cpu_count=cpu_count,
        explicit_gb=explicit_per_job_gb,
    )
    return max(1, int(global_gb // max(0.001, per_job)))


def plan_resource_pressure(
    *,
    prefix: str,
    environ: Mapping[str, str] | None = None,
    cpu_count: int | None = None,
    budget: memory_guard.AdaptiveMemoryBudget | None = None,
    max_cargo_build_jobs: int = DEFAULT_MAX_CARGO_BUILD_JOBS,
    cargo_build_gb_per_job: float = DEFAULT_CARGO_BUILD_GB_PER_JOB,
    max_compile_slots: int = DEFAULT_MAX_COMPILE_SLOTS,
    compile_gb_per_slot: float = DEFAULT_COMPILE_GB_PER_SLOT,
    compile_active_procs_factor: int = DEFAULT_COMPILE_ACTIVE_PROCS_FACTOR,
    compile_load_per_cpu: float = DEFAULT_COMPILE_LOAD_PER_CPU,
    diff_tree_gb: float | None = None,
    diff_global_gb: float | None = None,
    diff_mem_per_job_gb: float | None = None,
) -> ResourcePressurePlan:
    env = os.environ if environ is None else environ
    normalized_prefix = prefix.strip().upper().rstrip("_") or "MOLT"
    cpus = max(1, int(cpu_count if cpu_count is not None else (os.cpu_count() or 1)))
    memory_budget = budget or memory_guard.adaptive_memory_budget(
        normalized_prefix,
        env,
    )
    usable_gb = _usable_memory_gb(memory_budget)

    max_cargo = max(1, int(max_cargo_build_jobs))
    cargo_per_job = max(0.001, float(cargo_build_gb_per_job))
    cargo_memory_jobs = _memory_job_count(
        usable_gb,
        gb_per_job=cargo_per_job,
        fallback=min(2, max_cargo),
    )
    cargo_jobs = max(1, min(cpus, max_cargo, cargo_memory_jobs))

    compile_slots_cap = max(1, int(max_compile_slots))
    compile_per_slot = max(0.001, float(compile_gb_per_slot))
    compile_memory_slots = _memory_job_count(
        usable_gb,
        gb_per_job=compile_per_slot,
        fallback=min(DEFAULT_MAX_COMPILE_SLOTS, compile_slots_cap),
    )
    compile_slots = max(1, min(cpus, compile_slots_cap, compile_memory_slots))
    compile_active_procs = max(
        1,
        compile_slots * max(1, int(compile_active_procs_factor)),
    )
    compile_max_load = max(
        float(compile_slots),
        float(cpus) * max(0.0, float(compile_load_per_cpu)),
    )

    tree_gb = max(0.001, diff_tree_gb or memory_budget.max_total_rss_gb)
    global_gb = max(0.001, diff_global_gb or memory_budget.max_global_rss_gb)
    global_gb = max(0.001, global_gb)
    tree_gb = min(tree_gb, global_gb)
    per_job_gb = scheduler_per_job_gb(
        global_gb=global_gb,
        tree_gb=tree_gb,
        cpu_count=cpus,
        explicit_gb=diff_mem_per_job_gb,
    )
    diff_jobs = scheduler_max_jobs(
        global_gb=global_gb,
        tree_gb=tree_gb,
        cpu_count=cpus,
        explicit_per_job_gb=diff_mem_per_job_gb,
    )

    measured_gb = memory_budget.available_gb
    measured_label = "available"
    if measured_gb is None:
        measured_gb = memory_budget.physical_gb
        measured_label = "physical"
    if measured_gb is None:
        memory_part = "memory=fallback"
    else:
        memory_part = (
            f"memory={measured_label}:{measured_gb:.2f}GB "
            f"reserve={memory_budget.reserve_gb:.2f}GB"
        )
    pressure = _pressure_level(memory_budget)
    reason = (
        f"cpu={cpus} pressure={pressure} {memory_part} "
        f"cargo_jobs={cargo_jobs}/{max_cargo} "
        f"cargo_per_job={cargo_per_job:.2f}GB "
        f"compile_slots={compile_slots}/{compile_slots_cap} "
        f"diff_jobs={diff_jobs} diff_per_job={per_job_gb:.2f}GB"
    )

    return ResourcePressurePlan(
        prefix=normalized_prefix,
        cpu_count=cpus,
        pressure_level=pressure,
        memory_source=memory_budget.source,
        physical_gb=memory_budget.physical_gb,
        available_gb=memory_budget.available_gb,
        reserve_gb=memory_budget.reserve_gb,
        usable_gb=usable_gb,
        cargo_build_jobs=cargo_jobs,
        cargo_build_gb_per_job=cargo_per_job,
        max_cargo_build_jobs=max_cargo,
        compile_max_slots=compile_slots,
        compile_gb_per_slot=compile_per_slot,
        compile_max_active_procs=compile_active_procs,
        compile_max_load=compile_max_load,
        diff_scheduler_per_job_gb=per_job_gb,
        diff_max_jobs=diff_jobs,
        diff_global_gb=global_gb,
        diff_tree_gb=tree_gb,
        reason=reason,
    )
