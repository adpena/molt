from __future__ import annotations

import json
from pathlib import Path
import importlib.util
import sys

import pytest


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


def test_policy_derives_cargo_memory_from_attested_receipt() -> None:
    module = _load_ci_resource_env()

    policy = module.load_ci_resource_policy()

    assert policy.max_jobs == 4
    assert policy.measured_peak_rss_bytes == 2_347_479_040
    assert policy.headroom_ratio == pytest.approx(0.40)
    assert policy.measurement_run_id == 29_646_901_351
    assert policy.gb_per_job == pytest.approx((2_347_479_040 / 1024**3) * 1.40)


def test_plan_uses_four_cargo_jobs_on_calibrated_hosted_runner_shape() -> None:
    module = _load_ci_resource_env()

    plan = module.plan_ci_resources(
        environ={},
        cpu_count=4,
        budget=_budget(module, physical_gb=16.0, available_gb=14.0, reserve_gb=1.0),
    )

    assert plan.cargo_build_jobs == 4
    assert plan.cargo_build_memory_source == "receipt-calibration"
    assert plan.cargo_build_measured_peak_rss_bytes == 2_347_479_040
    assert plan.cargo_build_headroom_ratio == pytest.approx(0.40)
    assert "cpu=4" in plan.reason
    assert "available:14.00GB" in plan.reason
    assert "cargo_memory=receipt-calibration:run-29646901351" in plan.reason
    assert plan.resource_plan.to_json_dict()["schema"] == "molt.resource_pressure.v2"


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
    assert plan.cargo_build_memory_source == "environment-override"
    assert plan.cargo_build_measured_peak_rss_bytes is None
    assert plan.cargo_build_headroom_ratio is None


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
    assert "CARGO_BUILD_JOBS=4\n" in text
    assert "MOLT_CI_RESOURCE_CPU_COUNT=4\n" in text
    assert "MOLT_CI_RESOURCE_REASON=cpu=4" in text
    plan_json = next(
        line.removeprefix("MOLT_CI_RESOURCE_PLAN_JSON=")
        for line in text.splitlines()
        if line.startswith("MOLT_CI_RESOURCE_PLAN_JSON=")
    )
    payload = json.loads(plan_json)
    assert payload["schema"] == "molt.resource_pressure.v2"
    assert payload["cargo"]["build_jobs"] == 4
    assert payload["cargo"]["memory_source"] == "receipt-calibration"
    assert payload["cargo"]["measured_peak_rss_bytes"] == 2_347_479_040
    assert payload["cargo"]["measurement_run_id"] == 29_646_901_351


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
    assert payload["schema"] == "molt.resource_pressure.v2"
    assert payload["cargo"]["build_jobs"] == 4


def test_policy_rejects_unattested_memory_shape(tmp_path: Path) -> None:
    module = _load_ci_resource_env()
    policy_path = tmp_path / "ci_resource_policy.toml"
    policy_path.write_text(
        """
schema = "molt.ci-resource-policy.v1"
[cargo_build]
max_jobs = 4
measured_peak_rss_bytes = 2347479040
headroom_ratio = 0.0
measurement_run_id = 29646901351
measurement_commit = "4002a0956af24736d39bc6b077045a1c278f0adc"
measurement_command = "python3 tools/run_cargo_test_truth.py"
""".strip(),
        encoding="utf-8",
    )

    with pytest.raises(ValueError, match="headroom_ratio"):
        module.load_ci_resource_policy(policy_path)


@pytest.mark.parametrize(
    "environ",
    [
        {"MOLT_CI_MAX_CARGO_BUILD_JOBS": "0"},
        {"MOLT_CI_CARGO_BUILD_GB_PER_JOB": "nan"},
    ],
)
def test_plan_rejects_invalid_resource_overrides(environ: dict[str, str]) -> None:
    module = _load_ci_resource_env()

    with pytest.raises(ValueError, match="positive"):
        module.plan_ci_resources(
            environ=environ,
            cpu_count=4,
            budget=_budget(
                module,
                physical_gb=16.0,
                available_gb=14.0,
                reserve_gb=1.0,
            ),
        )
