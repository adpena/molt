from __future__ import annotations

import pytest

from tools import memory_guard, resource_pressure


def _budget(
    *,
    physical_gb: float,
    available_gb: float,
    reserve_gb: float,
    max_process_gb: float | None = None,
    max_total_gb: float | None = None,
    max_global_gb: float | None = None,
):
    return memory_guard.AdaptiveMemoryBudget(
        max_process_rss_gb=max_process_gb or available_gb * 0.4,
        max_total_rss_gb=max_total_gb or available_gb * 0.5,
        max_global_rss_gb=max_global_gb or available_gb * 0.8,
        reserve_gb=reserve_gb,
        physical_gb=physical_gb,
        available_gb=available_gb,
        source="test",
    )


def test_resource_pressure_json_contract_scales_dev_policies() -> None:
    plan = resource_pressure.plan_resource_pressure(
        prefix="MOLT_CI",
        environ={},
        cpu_count=12,
        budget=_budget(
            physical_gb=128.0,
            available_gb=96.0,
            reserve_gb=7.68,
            max_total_gb=51.40224,
            max_global_gb=85.6704,
        ),
        max_cargo_build_jobs=4,
        cargo_build_gb_per_job=12.0,
        max_compile_slots=2,
    )

    payload = plan.to_json_dict()

    assert payload["schema"] == resource_pressure.SCHEMA_VERSION
    assert plan.pressure_level == "low"
    assert plan.cargo_build_jobs == 4
    assert plan.compile_max_slots == 2
    assert plan.diff_scheduler_per_job_gb == pytest.approx(7.1392)
    assert plan.diff_max_jobs == 12
    assert "cpu=12" in plan.reason
    assert isinstance(payload["memory"], dict)
    assert payload["memory"]["available_gb"] == 96.0


def test_resource_pressure_reduces_defaults_under_memory_pressure() -> None:
    plan = resource_pressure.plan_resource_pressure(
        prefix="MOLT_COMPILE_GUARD",
        environ={},
        cpu_count=16,
        budget=_budget(physical_gb=64.0, available_gb=8.0, reserve_gb=4.0),
        max_cargo_build_jobs=8,
        cargo_build_gb_per_job=6.0,
        max_compile_slots=4,
    )

    assert plan.pressure_level == "high"
    assert plan.cargo_build_jobs == 1
    assert plan.compile_max_slots == 1
    assert plan.compile_max_active_procs == 3


def test_diff_scheduler_helpers_share_policy_with_plan() -> None:
    assert resource_pressure.scheduler_per_job_gb(
        global_gb=85.6704,
        tree_gb=51.40224,
        cpu_count=12,
    ) == pytest.approx(7.1392)
    assert (
        resource_pressure.scheduler_max_jobs(
            global_gb=23.5904,
            tree_gb=14.15424,
            cpu_count=64,
        )
        == 23
    )
