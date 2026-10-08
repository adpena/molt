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
schema = "molt.ci-resource-policy.v2"
[cargo_build]
max_jobs = 4
measured_peak_rss_bytes = 2347479040
headroom_ratio = 0.0
measurement_run_id = 29646901351
measurement_commit = "c299b9e8cdf4537a389d6f93760951398c5f3c0a"
measurement_command = "python3 tools/run_cargo_test_truth.py"

[cargo_environment]
incident_run_id = 30211145633
incident_job_id = 89817499999
incident_commit = "20b046b79bd4ca64a8c859f737f6e330377bcc4e"
incident_command = "cargo metadata --locked --format-version 1"

[cargo_execution]
cross_check_timeout_seconds = 240
warm_timeout_seconds = 300
integration_timeout_seconds = 600
cold_timeout_seconds = 1200
suite_timeout_seconds = 1800
shipping_timeout_seconds = 9000
observed_cold_timeout_seconds = 300.51
minimum_cold_headroom_multiplier = 3.0
measurement_run_id = 30209686001
measurement_job_id = 89813773652
measurement_commit = "adcc350d6ed1fb8541bb0202e7bc4b248cd9a8c4"
measurement_command = "cargo build --locked --profile dev-fast -p molt-wasm-host"
""".strip(),
        encoding="utf-8",
    )

    with pytest.raises(ValueError, match="headroom_ratio"):
        module.load_ci_resource_policy(policy_path)


def test_cargo_execution_budget_is_receipt_calibrated() -> None:
    module = _load_ci_resource_env()

    policy = module.load_ci_cargo_policy().execution_budgets

    assert dict(policy.timeout_seconds_by_class) == {
        "cross-check": 240,
        "warm": 300,
        "integration": 600,
        "cold": 1200,
        "suite": 1800,
        "shipping": 9000,
    }
    assert policy.observed_cold_timeout_seconds == pytest.approx(300.51)
    assert policy.minimum_cold_headroom_multiplier == pytest.approx(3.0)
    assert policy.measurement_run_id == 30_209_686_001
    assert policy.measurement_job_id == 89_813_773_652


def test_cargo_environment_policy_retains_native_incident_receipt() -> None:
    module = _load_ci_resource_env()

    policy = module.load_ci_cargo_policy().environment

    assert policy.wrapper_environment_names == (
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    )
    assert policy.incident_run_id == 30_211_145_633
    assert policy.incident_job_id == 89_817_499_999
    assert policy.incident_commit == "20b046b79bd4ca64a8c859f737f6e330377bcc4e"
    assert policy.incident_command == "cargo metadata --locked --format-version 1"


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


def test_guard_caps_admit_the_calibrated_need_of_the_job_a_small_runner_plans() -> None:
    # The 7 GB macOS runner: 3.22 GB available, 1 GB reserve. Capping the one
    # admitted Cargo job at the 2.22 GB usable snapshot killed it at 2.23 GB,
    # below its receipt-calibrated 3.06 GB need.
    module = _load_ci_resource_env()
    budget = _budget(module, physical_gb=7.0, available_gb=3.22, reserve_gb=1.0)
    plan = module.plan_ci_resources(environ={}, cpu_count=3, budget=budget)

    assert plan.cargo_build_jobs == 1
    need = plan.cargo_build_gb_per_job
    assert need > plan.resource_plan.usable_gb
    assert plan.guard_max_process_rss_gb == pytest.approx(need)
    assert plan.guard_max_total_rss_gb == pytest.approx(need)


def test_guard_caps_stay_below_physical_memory_less_the_reserve() -> None:
    # A runner too small for even the calibrated need: the caps stop at the
    # host ceiling rather than at the need.
    module = _load_ci_resource_env()
    budget = _budget(module, physical_gb=3.0, available_gb=2.5, reserve_gb=1.0)
    plan = module.plan_ci_resources(environ={}, cpu_count=2, budget=budget)

    assert plan.cargo_build_gb_per_job > 2.0
    assert plan.guard_max_process_rss_gb == pytest.approx(2.0)
    assert plan.guard_max_total_rss_gb == pytest.approx(2.0)


def test_guard_caps_never_fall_below_the_guard_defaults() -> None:
    # A 16 GB runner with four jobs: usable / jobs is below the guard's own
    # process cap, which stays.
    module = _load_ci_resource_env()
    budget = _budget(module, physical_gb=16.0, available_gb=14.0, reserve_gb=1.0)
    plan = module.plan_ci_resources(environ={}, cpu_count=4, budget=budget)

    assert plan.cargo_build_jobs == 4
    assert plan.guard_max_process_rss_gb == pytest.approx(budget.max_process_rss_gb)
    assert plan.guard_max_total_rss_gb == pytest.approx(budget.max_total_rss_gb)


def test_github_env_carries_the_guard_caps(tmp_path: Path) -> None:
    module = _load_ci_resource_env()
    plan = module.plan_ci_resources(
        environ={},
        cpu_count=3,
        budget=_budget(module, physical_gb=7.0, available_gb=3.22, reserve_gb=1.0),
    )
    env_file = tmp_path / "github-env"
    module.write_github_env(env_file, plan)
    lines = dict(
        line.split("=", 1) for line in env_file.read_text(encoding="utf-8").splitlines()
    )
    assert float(lines["MOLT_MAX_PROCESS_RSS_GB"]) == pytest.approx(
        plan.guard_max_process_rss_gb
    )
    assert float(lines["MOLT_MAX_TOTAL_RSS_GB"]) == pytest.approx(
        plan.guard_max_total_rss_gb
    )


@pytest.mark.parametrize(
    ("physical_gb", "available_gb", "cpus"),
    [(7.0, 3.22, 3), (16.0, 14.25, 4)],
)
def test_github_env_sizes_pytest_auto_workers_from_the_plan(
    tmp_path: Path, physical_gb: float, available_gb: float, cpus: int
) -> None:
    # `pytest -n auto` reads this variable, so one plan sizes test workers.
    module = _load_ci_resource_env()
    plan = module.plan_ci_resources(
        environ={},
        cpu_count=cpus,
        budget=_budget(
            module, physical_gb=physical_gb, available_gb=available_gb, reserve_gb=1.0
        ),
    )
    env_file = tmp_path / "github-env"
    module.write_github_env(env_file, plan)
    lines = dict(
        line.split("=", 1) for line in env_file.read_text(encoding="utf-8").splitlines()
    )
    workers = int(lines["PYTEST_XDIST_AUTO_NUM_WORKERS"])
    assert workers == plan.resource_plan.diff_max_jobs
    assert 1 <= workers <= cpus


def test_darwin_breakdown_names_every_page_class_and_the_counted_ones() -> None:
    module = _load_ci_resource_env()
    text = "\n".join(
        [
            "Mach Virtual Memory Statistics: (page size of 16384 bytes)",
            "Pages free:                               65536.",
            "Pages active:                            131072.",
            "Pages inactive:                           65536.",
            "Pages speculative:                        32768.",
            "Pages wired down:                         16384.",
            "Pages purgeable:                           8192.",
            "File-backed pages:                       98304.",
            "Anonymous pages:                         131072.",
            "Pages occupied by compressor:              4096.",
        ]
    )

    assert module.darwin_memory_breakdown(text) == (
        "macOS memory pages (GiB): free=1.00 inactive=1.00 speculative=0.50 "
        "purgeable=0.12 active=2.00 wired=0.25 file-backed=1.50 anonymous=2.00 "
        "compressor=0.06; available counts free+inactive+speculative+purgeable"
    )
    assert module.darwin_memory_breakdown("no page size here") is None
