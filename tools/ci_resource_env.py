#!/usr/bin/env python3
from __future__ import annotations

import argparse
from collections.abc import Mapping
from dataclasses import dataclass
import os
from pathlib import Path

try:
    from tools import memory_guard
except ModuleNotFoundError:  # pragma: no cover - direct script execution
    import memory_guard  # type: ignore


DEFAULT_MAX_CARGO_BUILD_JOBS = 4
DEFAULT_CARGO_BUILD_GB_PER_JOB = 12.0


@dataclass(frozen=True, slots=True)
class CiResourcePlan:
    cargo_build_jobs: int
    cpu_count: int
    max_cargo_build_jobs: int
    cargo_build_gb_per_job: float
    physical_gb: float | None
    available_gb: float | None
    reserve_gb: float
    reason: str


def _positive_int(raw: str | None, *, default: int) -> int:
    if raw is None or not raw.strip():
        return default
    try:
        value = int(raw)
    except ValueError:
        return default
    return value if value > 0 else default


def _positive_float(raw: str | None, *, default: float) -> float:
    if raw is None or not raw.strip():
        return default
    try:
        value = float(raw)
    except ValueError:
        return default
    return value if value > 0 else default


def plan_ci_resources(
    *,
    environ: Mapping[str, str] | None = None,
    cpu_count: int | None = None,
    budget: memory_guard.AdaptiveMemoryBudget | None = None,
) -> CiResourcePlan:
    env = os.environ if environ is None else environ
    cpus = max(1, int(cpu_count if cpu_count is not None else (os.cpu_count() or 1)))
    max_jobs = _positive_int(
        env.get("MOLT_CI_MAX_CARGO_BUILD_JOBS"),
        default=DEFAULT_MAX_CARGO_BUILD_JOBS,
    )
    gb_per_job = _positive_float(
        env.get("MOLT_CI_CARGO_BUILD_GB_PER_JOB"),
        default=DEFAULT_CARGO_BUILD_GB_PER_JOB,
    )
    memory_budget = budget or memory_guard.adaptive_memory_budget("MOLT_CI", env)
    measured_gb = memory_budget.available_gb
    measured_label = "available"
    if measured_gb is None:
        measured_gb = memory_budget.physical_gb
        measured_label = "physical"

    if measured_gb is None:
        memory_jobs = min(2, max_jobs)
        memory_part = "memory=fallback"
    else:
        usable_gb = max(0.0, measured_gb - memory_budget.reserve_gb)
        memory_jobs = max(1, int(usable_gb // gb_per_job))
        memory_part = (
            f"memory={measured_label}:{measured_gb:.2f}GB "
            f"reserve={memory_budget.reserve_gb:.2f}GB "
            f"per_job={gb_per_job:.2f}GB"
        )

    jobs = max(1, min(cpus, max_jobs, memory_jobs))
    reason = f"cpu={cpus} max={max_jobs} {memory_part}"
    return CiResourcePlan(
        cargo_build_jobs=jobs,
        cpu_count=cpus,
        max_cargo_build_jobs=max_jobs,
        cargo_build_gb_per_job=gb_per_job,
        physical_gb=memory_budget.physical_gb,
        available_gb=memory_budget.available_gb,
        reserve_gb=memory_budget.reserve_gb,
        reason=reason,
    )


def _github_env_lines(plan: CiResourcePlan) -> list[str]:
    return [
        f"CARGO_BUILD_JOBS={plan.cargo_build_jobs}",
        f"MOLT_CI_RESOURCE_CPU_COUNT={plan.cpu_count}",
        f"MOLT_CI_RESOURCE_REASON={plan.reason}",
    ]


def write_github_env(path: Path, plan: CiResourcePlan) -> None:
    with path.open("a", encoding="utf-8") as handle:
        for line in _github_env_lines(plan):
            handle.write(f"{line}\n")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Emit adaptive CI resource defaults for GitHub Actions jobs."
    )
    parser.add_argument("--github-env", type=Path)
    args = parser.parse_args(argv)

    plan = plan_ci_resources()
    if args.github_env is not None:
        write_github_env(args.github_env, plan)
    print(
        f"Configured CARGO_BUILD_JOBS={plan.cargo_build_jobs} ({plan.reason})",
        flush=True,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
