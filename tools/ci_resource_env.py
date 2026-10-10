#!/usr/bin/env python3
from __future__ import annotations

import argparse
from collections.abc import Mapping
from dataclasses import dataclass
import json
import math
import os
from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[1]
_SRC_ROOT = ROOT / "src"
if str(_SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(_SRC_ROOT))

from molt.cargo_execution_policy import (  # noqa: E402
    CargoBuildResourcePolicy,
    load_ci_cargo_policy,
)

if __package__:
    from . import memory_guard, resource_pressure
else:  # pragma: no cover - direct script execution
    import memory_guard  # type: ignore
    import resource_pressure  # type: ignore


CI_RESOURCE_POLICY = ROOT / "config" / "ci_resource_policy.toml"
CargoBuildMemoryPolicy = CargoBuildResourcePolicy


def load_ci_resource_policy(
    path: Path = CI_RESOURCE_POLICY,
) -> CargoBuildMemoryPolicy:
    return load_ci_cargo_policy(path).build_resources


@dataclass(frozen=True, slots=True)
class CiResourcePlan:
    cargo_build_jobs: int
    cpu_count: int
    max_cargo_build_jobs: int
    cargo_build_gb_per_job: float
    cargo_build_memory_source: str
    cargo_build_measured_peak_rss_bytes: int | None
    cargo_build_headroom_ratio: float | None
    cargo_build_measurement_run_id: int | None
    cargo_build_measurement_commit: str | None
    cargo_build_measurement_command: str | None
    physical_gb: float | None
    available_gb: float | None
    reserve_gb: float
    reason: str
    resource_plan: resource_pressure.ResourcePressurePlan
    guard_max_process_rss_gb: float | None
    guard_max_total_rss_gb: float | None
    python_test_workers: int


def _positive_int(raw: str | None, *, default: int) -> int:
    if raw is None or not raw.strip():
        return default
    try:
        value = int(raw)
    except ValueError as exc:
        raise ValueError(f"expected a positive integer, got {raw!r}") from exc
    if value <= 0:
        raise ValueError(f"expected a positive integer, got {raw!r}")
    return value


def _positive_float(raw: str | None, *, default: float) -> float:
    if raw is None or not raw.strip():
        return default
    try:
        value = float(raw)
    except ValueError as exc:
        raise ValueError(f"expected a positive finite number, got {raw!r}") from exc
    if not math.isfinite(value) or value <= 0:
        raise ValueError(f"expected a positive finite number, got {raw!r}")
    return value


def plan_ci_resources(
    *,
    environ: Mapping[str, str] | None = None,
    cpu_count: int | None = None,
    budget: memory_guard.AdaptiveMemoryBudget | None = None,
) -> CiResourcePlan:
    env = os.environ if environ is None else environ
    cpus = max(1, int(cpu_count if cpu_count is not None else (os.cpu_count() or 1)))
    policy = load_ci_resource_policy()
    max_jobs = _positive_int(
        env.get("MOLT_CI_MAX_CARGO_BUILD_JOBS"),
        default=policy.max_jobs,
    )
    raw_gb_per_job = env.get("MOLT_CI_CARGO_BUILD_GB_PER_JOB")
    if raw_gb_per_job is None or not raw_gb_per_job.strip():
        gb_per_job = policy.gb_per_job
        memory_source = "receipt-calibration"
        measured_peak_rss_bytes: int | None = policy.measured_peak_rss_bytes
        headroom_ratio: float | None = policy.headroom_ratio
        measurement_run_id: int | None = policy.measurement_run_id
        measurement_commit: str | None = policy.measurement_commit
        measurement_command: str | None = policy.measurement_command
    else:
        gb_per_job = _positive_float(raw_gb_per_job, default=policy.gb_per_job)
        memory_source = "environment-override"
        measured_peak_rss_bytes = None
        headroom_ratio = None
        measurement_run_id = None
        measurement_commit = None
        measurement_command = None
    memory_budget = budget or memory_guard.adaptive_memory_budget("MOLT_CI", env)
    pressure_plan = resource_pressure.plan_resource_pressure(
        prefix="MOLT_CI",
        environ=env,
        cpu_count=cpus,
        budget=memory_budget,
        max_cargo_build_jobs=max_jobs,
        cargo_build_gb_per_job=gb_per_job,
        cargo_build_memory_source=memory_source,
        cargo_build_measured_peak_rss_bytes=measured_peak_rss_bytes,
        cargo_build_headroom_ratio=headroom_ratio,
        cargo_build_measurement_run_id=measurement_run_id,
        cargo_build_measurement_commit=measurement_commit,
        cargo_build_measurement_command=measurement_command,
    )
    process_cap, total_cap = _guard_caps(memory_budget, pressure_plan)
    return CiResourcePlan(
        cargo_build_jobs=pressure_plan.cargo_build_jobs,
        cpu_count=cpus,
        max_cargo_build_jobs=max_jobs,
        cargo_build_gb_per_job=gb_per_job,
        cargo_build_memory_source=memory_source,
        cargo_build_measured_peak_rss_bytes=measured_peak_rss_bytes,
        cargo_build_headroom_ratio=headroom_ratio,
        cargo_build_measurement_run_id=measurement_run_id,
        cargo_build_measurement_commit=measurement_commit,
        cargo_build_measurement_command=measurement_command,
        physical_gb=memory_budget.physical_gb,
        available_gb=memory_budget.available_gb,
        reserve_gb=memory_budget.reserve_gb,
        reason=pressure_plan.reason,
        resource_plan=pressure_plan,
        guard_max_process_rss_gb=process_cap,
        guard_max_total_rss_gb=total_cap,
        # `pytest -n auto` reads this count from the exported variable. Workers
        # use the differential scheduler's per-job estimate within the global
        # budget, the count the queue suites' budgets were calibrated against
        # (be612afe9: 2 on the 7 GB macOS runner, 4 on Linux). Differential
        # jobs, which build and run compiled programs, are bounded by the tree
        # budget instead (diff_max_jobs, HF-105).
        python_test_workers=max(
            1,
            int(
                pressure_plan.diff_global_gb
                // max(0.001, pressure_plan.diff_scheduler_per_job_gb)
            ),
        ),
    )


def _guard_caps(
    budget: memory_guard.AdaptiveMemoryBudget,
    plan: resource_pressure.ResourcePressurePlan,
) -> tuple[float | None, float | None]:
    """The memory guard caps that admit every job this plan admits.

    Each admitted Cargo job may use its share of usable memory and at least its
    receipt-calibrated need. A small runner still admits one job when that need
    exceeds the usable snapshot (7 GB macOS: 2.2 GB usable, 3.06 GB calibrated),
    and a cap below the need killed that job at 2.23 GB. The host ceiling is
    physical memory less the reserve; the guard's own dynamic global budget
    still protects the host at run time. The caps never fall below the guard's
    defaults, which the guard clamps to its hard and global ceilings.
    """
    usable = plan.usable_gb
    if usable is None or usable <= 0:
        return None, None
    ceiling = (
        max(usable, plan.physical_gb - plan.reserve_gb)
        if plan.physical_gb is not None
        else usable
    )
    per_job = max(usable / max(1, plan.cargo_build_jobs), plan.cargo_build_gb_per_job)
    process_cap = min(ceiling, max(budget.max_process_rss_gb, per_job))
    total_cap = min(ceiling, max(budget.max_total_rss_gb, process_cap))
    return process_cap, total_cap


def github_env_lines(plan: CiResourcePlan) -> list[str]:
    plan_json = json.dumps(
        plan.resource_plan.to_json_dict(),
        sort_keys=True,
        separators=(",", ":"),
    )
    lines = [
        f"CARGO_BUILD_JOBS={plan.cargo_build_jobs}",
        f"MOLT_CI_RESOURCE_CPU_COUNT={plan.cpu_count}",
        f"MOLT_CI_RESOURCE_REASON={plan.reason}",
        f"MOLT_CI_RESOURCE_PLAN_JSON={plan_json}",
        f"PYTEST_XDIST_AUTO_NUM_WORKERS={plan.python_test_workers}",
    ]
    if plan.guard_max_process_rss_gb is not None:
        lines.append(f"MOLT_MAX_PROCESS_RSS_GB={plan.guard_max_process_rss_gb:.6f}")
    if plan.guard_max_total_rss_gb is not None:
        lines.append(f"MOLT_MAX_TOTAL_RSS_GB={plan.guard_max_total_rss_gb:.6f}")
    return lines


def write_github_env(path: Path, plan: CiResourcePlan) -> None:
    with path.open("a", encoding="utf-8") as handle:
        for line in github_env_lines(plan):
            handle.write(f"{line}\n")


def darwin_memory_breakdown(vm_stat_text: str) -> str | None:
    """Name the macOS page classes behind the plan's available-memory figure.

    The guard counts only some page classes as available; logging all of them
    lets a small runner's budget be judged from evidence (HF-68).
    """
    parsed = memory_guard.parse_darwin_vm_stat(vm_stat_text)
    if parsed is None:
        return None
    page_size, pages = parsed
    rows = (
        ("free", "Pages free"),
        ("inactive", "Pages inactive"),
        ("speculative", "Pages speculative"),
        ("purgeable", "Pages purgeable"),
        ("active", "Pages active"),
        ("wired", "Pages wired down"),
        ("file-backed", "File-backed pages"),
        ("anonymous", "Anonymous pages"),
        ("compressor", "Pages occupied by compressor"),
    )
    parts = [
        f"{label}={pages[name] * page_size / 2**30:.2f}"
        for label, name in rows
        if name in pages
    ]
    counted = "+".join(
        label for label, name in rows if name in memory_guard.DARWIN_AVAILABLE_PAGE_ROWS
    )
    return f"macOS memory pages (GiB): {' '.join(parts)}; available counts {counted}"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Emit adaptive CI resource defaults for GitHub Actions jobs."
    )
    parser.add_argument("--github-env", type=Path)
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="compute and print the plan without writing --github-env",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="print the stable resource-pressure JSON payload",
    )
    args = parser.parse_args(argv)

    plan = plan_ci_resources()
    if args.github_env is not None and not args.dry_run:
        write_github_env(args.github_env, plan)
    if args.json:
        print(json.dumps(plan.resource_plan.to_json_dict(), sort_keys=True))
    else:
        print(
            f"Configured CARGO_BUILD_JOBS={plan.cargo_build_jobs} ({plan.reason})",
            flush=True,
        )
        if sys.platform == "darwin":
            text = memory_guard.darwin_vm_stat_text()
            breakdown = None if text is None else darwin_memory_breakdown(text)
            if breakdown is not None:
                print(breakdown, flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
