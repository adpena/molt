from __future__ import annotations

import json
from pathlib import Path
import importlib.util
import sys


REPO_ROOT = Path(__file__).resolve().parents[2]
CI_RESOURCE_ENV = REPO_ROOT / "tools" / "ci_resource_env.py"


def _load_ci_resource_env():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_ci_resource_env",
        CI_RESOURCE_ENV,
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _budget(module, *, physical_gb: float, available_gb: float, reserve_gb: float):
    return module.memory_guard.AdaptiveMemoryBudget(
        max_process_rss_gb=available_gb * 0.4,
        max_total_rss_gb=available_gb * 0.5,
        max_global_rss_gb=available_gb * 0.8,
        reserve_gb=reserve_gb,
        physical_gb=physical_gb,
        available_gb=available_gb,
        source="test",
    )


def test_plan_uses_one_cargo_job_on_default_hosted_runner_shape() -> None:
    module = _load_ci_resource_env()

    plan = module.plan_ci_resources(
        environ={},
        cpu_count=4,
        budget=_budget(module, physical_gb=16.0, available_gb=14.0, reserve_gb=1.0),
    )

    assert plan.cargo_build_jobs == 1
    assert "cpu=4" in plan.reason
    assert "available:14.00GB" in plan.reason
    assert plan.resource_plan.to_json_dict()["schema"] == "molt.resource_pressure.v1"


def test_plan_clamps_to_one_job_when_memory_is_pressured() -> None:
    module = _load_ci_resource_env()

    plan = module.plan_ci_resources(
        environ={},
        cpu_count=8,
        budget=_budget(module, physical_gb=16.0, available_gb=6.0, reserve_gb=1.0),
    )

    assert plan.cargo_build_jobs == 1


def test_plan_allows_larger_self_hosted_runners_with_explicit_cap() -> None:
    module = _load_ci_resource_env()

    plan = module.plan_ci_resources(
        environ={
            "MOLT_CI_MAX_CARGO_BUILD_JOBS": "8",
            "MOLT_CI_CARGO_BUILD_GB_PER_JOB": "4",
        },
        cpu_count=16,
        budget=_budget(module, physical_gb=64.0, available_gb=48.0, reserve_gb=4.0),
    )

    assert plan.cargo_build_jobs == 8


def test_write_github_env_emits_cargo_jobs_and_resource_reason(tmp_path: Path) -> None:
    module = _load_ci_resource_env()
    env_path = tmp_path / "github_env"
    plan = module.plan_ci_resources(
        environ={},
        cpu_count=4,
        budget=_budget(module, physical_gb=16.0, available_gb=14.0, reserve_gb=1.0),
    )

    module.write_github_env(env_path, plan)

    text = env_path.read_text(encoding="utf-8")
    assert "CARGO_BUILD_JOBS=1\n" in text
    assert "MOLT_CI_RESOURCE_CPU_COUNT=4\n" in text
    assert "MOLT_CI_RESOURCE_REASON=cpu=4" in text
    plan_json = next(
        line.removeprefix("MOLT_CI_RESOURCE_PLAN_JSON=")
        for line in text.splitlines()
        if line.startswith("MOLT_CI_RESOURCE_PLAN_JSON=")
    )
    payload = json.loads(plan_json)
    assert payload["schema"] == "molt.resource_pressure.v1"
    assert payload["cargo"]["build_jobs"] == 1


def test_main_json_dry_run_does_not_write_github_env(
    tmp_path: Path,
    monkeypatch,
    capsys,
) -> None:
    module = _load_ci_resource_env()
    env_path = tmp_path / "github_env"
    budget = _budget(module, physical_gb=16.0, available_gb=14.0, reserve_gb=1.0)
    monkeypatch.setattr(
        module.memory_guard, "adaptive_memory_budget", lambda *a: budget
    )
    monkeypatch.setattr(module.os, "cpu_count", lambda: 4)

    assert module.main(["--github-env", str(env_path), "--dry-run", "--json"]) == 0

    assert not env_path.exists()
    payload = json.loads(capsys.readouterr().out)
    assert payload["schema"] == "molt.resource_pressure.v1"
    assert payload["cargo"]["build_jobs"] == 1
