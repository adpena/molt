#!/usr/bin/env python3
from __future__ import annotations

import argparse
from collections.abc import Mapping
from dataclasses import dataclass
import json
import math
import os
from pathlib import Path
import re
import tomllib

if __package__:
    from . import memory_guard, resource_pressure
else:  # pragma: no cover - direct script execution
    import memory_guard  # type: ignore
    import resource_pressure  # type: ignore


ROOT = Path(__file__).resolve().parents[1]
CI_RESOURCE_POLICY = ROOT / "config" / "ci_resource_policy.toml"
CI_RESOURCE_POLICY_SCHEMA = "molt.ci-resource-policy.v1"


@dataclass(frozen=True, slots=True)
class CargoBuildMemoryPolicy:
    max_jobs: int
    measured_peak_rss_bytes: int
    headroom_ratio: float
    measurement_run_id: int
    measurement_commit: str
    measurement_command: str

    @property
    def gb_per_job(self) -> float:
        measured_gib = self.measured_peak_rss_bytes / float(1024**3)
        return measured_gib * (1.0 + self.headroom_ratio)


def load_ci_resource_policy(
    path: Path = CI_RESOURCE_POLICY,
) -> CargoBuildMemoryPolicy:
    try:
        payload = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise ValueError(f"cannot read CI resource policy {path}: {exc}") from exc
    if payload.get("schema") != CI_RESOURCE_POLICY_SCHEMA:
        raise ValueError(
            f"CI resource policy schema must be {CI_RESOURCE_POLICY_SCHEMA!r}"
        )
    cargo = payload.get("cargo_build")
    expected_fields = {
        "max_jobs",
        "measured_peak_rss_bytes",
        "headroom_ratio",
        "measurement_run_id",
        "measurement_commit",
        "measurement_command",
    }
    if not isinstance(cargo, Mapping) or set(cargo) != expected_fields:
        raise ValueError(
            "CI resource policy cargo_build fields must be exactly "
            + ", ".join(sorted(expected_fields))
        )
    max_jobs = cargo["max_jobs"]
    measured_peak = cargo["measured_peak_rss_bytes"]
    headroom = cargo["headroom_ratio"]
    run_id = cargo["measurement_run_id"]
    commit = cargo["measurement_commit"]
    command = cargo["measurement_command"]
    if not isinstance(max_jobs, int) or isinstance(max_jobs, bool) or max_jobs <= 0:
        raise ValueError("CI resource policy max_jobs must be a positive integer")
    if (
        not isinstance(measured_peak, int)
        or isinstance(measured_peak, bool)
        or measured_peak <= 0
    ):
        raise ValueError(
            "CI resource policy measured_peak_rss_bytes must be a positive integer"
        )
    if (
        not isinstance(headroom, (int, float))
        or isinstance(headroom, bool)
        or not math.isfinite(float(headroom))
        or not 0.10 <= float(headroom) <= 2.0
    ):
        raise ValueError(
            "CI resource policy headroom_ratio must be finite and within [0.10, 2.0]"
        )
    if not isinstance(run_id, int) or isinstance(run_id, bool) or run_id <= 0:
        raise ValueError(
            "CI resource policy measurement_run_id must be a positive integer"
        )
    if not isinstance(commit, str) or re.fullmatch(r"[0-9a-f]{40}", commit) is None:
        raise ValueError(
            "CI resource policy measurement_commit must be a lowercase 40-hex SHA"
        )
    if not isinstance(command, str) or not command.strip():
        raise ValueError(
            "CI resource policy measurement_command must be a non-empty string"
        )
    return CargoBuildMemoryPolicy(
        max_jobs=max_jobs,
        measured_peak_rss_bytes=measured_peak,
        headroom_ratio=float(headroom),
        measurement_run_id=run_id,
        measurement_commit=commit,
        measurement_command=command,
    )


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
    )


def _github_env_lines(plan: CiResourcePlan) -> list[str]:
    plan_json = json.dumps(
        plan.resource_plan.to_json_dict(),
        sort_keys=True,
        separators=(",", ":"),
    )
    return [
        f"CARGO_BUILD_JOBS={plan.cargo_build_jobs}",
        f"MOLT_CI_RESOURCE_CPU_COUNT={plan.cpu_count}",
        f"MOLT_CI_RESOURCE_REASON={plan.reason}",
        f"MOLT_CI_RESOURCE_PLAN_JSON={plan_json}",
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
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
