"""Shared Cargo subprocess, resource, and timeout authority."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
import math
import os
from pathlib import Path
import re
import tomllib


CI_CARGO_POLICY_SCHEMA = "molt.ci-resource-policy.v2"
DEFAULT_CI_CARGO_POLICY = (
    Path(__file__).resolve().parents[2] / "config" / "ci_resource_policy.toml"
)
SCCACHE_INCREMENTAL_POLICY = "sccache-disables-incremental"
DIRECT_RUSTC_INCREMENTAL_POLICY = "direct-rustc-enables-incremental"
CARGO_WRAPPER_ENV_NAMES = (
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
)
PROOF_COMMAND_TIMEOUT_ENV = "MOLT_PROOF_COMMAND_TIMEOUT_SEC"
NESTED_PROCESS_BUDGET_BY_ROLE = {
    "build": "cold",
    "execution": "warm",
}


def _wrapper_is_sccache(wrapper: str) -> bool:
    name = wrapper.strip().strip('"').replace("\\", "/").rsplit("/", 1)[-1]
    return name.lower() in {"sccache", "sccache.exe"}


def cargo_compiler_wrappers(environ: Mapping[str, str]) -> tuple[tuple[str, str], ...]:
    return tuple(
        (name, value)
        for name in CARGO_WRAPPER_ENV_NAMES
        if (value := environ.get(name, "").strip())
    )


def sccache_compiler_wrappers(
    environ: Mapping[str, str],
) -> tuple[tuple[str, str], ...]:
    return tuple(
        (name, value)
        for name, value in cargo_compiler_wrappers(environ)
        if _wrapper_is_sccache(value)
    )


def normalize_cargo_environment(
    environ: Mapping[str, str] | None,
    *,
    default_incremental: str | None = None,
) -> tuple[dict[str, str], tuple[str, ...]]:
    """Return one executable Cargo environment and its applied policy names.

    Cargo accepts compiler wrappers through both direct Rust variables and
    ``build.*`` configuration environment variables.  Every subprocess boundary
    must inspect the complete family: checking only ``RUSTC_WRAPPER`` leaves
    probes and workspace wrappers able to inherit the invalid
    sccache-plus-incremental combination.
    """

    child = dict(os.environ) if environ is None else dict(environ)
    if sccache_compiler_wrappers(child):
        child["CARGO_INCREMENTAL"] = "0"
        return child, (SCCACHE_INCREMENTAL_POLICY,)
    if default_incremental is not None and "CARGO_INCREMENTAL" not in child:
        child["CARGO_INCREMENTAL"] = default_incremental
        return child, (DIRECT_RUSTC_INCREMENTAL_POLICY,)
    return child, ()


def without_sccache_compiler_wrappers(
    environ: Mapping[str, str],
) -> dict[str, str]:
    child = dict(environ)
    for name, _value in sccache_compiler_wrappers(child):
        child.pop(name, None)
    return child


def _executable_name(value: str) -> str:
    name = value.replace("\\", "/").rsplit("/", 1)[-1].lower()
    return name[:-4] if name.endswith(".exe") else name


def is_cargo_command(command: Sequence[str]) -> bool:
    if not command:
        return False
    if _executable_name(str(command[0])) == "cargo":
        return True
    if _executable_name(str(command[0])) != "rustup":
        return False
    return any(_executable_name(str(part)) == "cargo" for part in command[1:])


def cargo_subprocess_environment(
    command: Sequence[str],
    environ: Mapping[str, str] | None,
    *,
    default_incremental: str | None = None,
) -> tuple[Mapping[str, str] | None, tuple[str, ...]]:
    if not is_cargo_command(command):
        return environ, ()
    return normalize_cargo_environment(
        environ,
        default_incremental=default_incremental,
    )


def proof_command_timeout_seconds(
    environ: Mapping[str, str],
) -> float | None:
    raw = environ.get(PROOF_COMMAND_TIMEOUT_ENV)
    if raw is None or not raw.strip():
        return None
    try:
        value = float(raw)
    except ValueError as exc:
        raise ValueError(
            f"{PROOF_COMMAND_TIMEOUT_ENV} must be a positive finite number"
        ) from exc
    if not math.isfinite(value) or value <= 0.0:
        raise ValueError(
            f"{PROOF_COMMAND_TIMEOUT_ENV} must be a positive finite number"
        )
    return value


def default_nested_process_timeout_seconds(role: str) -> float:
    try:
        budget_class = NESTED_PROCESS_BUDGET_BY_ROLE[role]
    except KeyError as exc:
        choices = ", ".join(sorted(NESTED_PROCESS_BUDGET_BY_ROLE))
        raise ValueError(
            f"unknown nested process role {role!r}; expected {choices}"
        ) from exc
    return float(load_ci_cargo_policy().execution_budgets.timeout_seconds(budget_class))


def enforce_owning_proof_timeout(
    environ: Mapping[str, str],
    selected_timeout: float | None,
) -> float | None:
    """Keep a default/env-selected inner guard from undercutting its owner.

    Callers resolve an explicit function argument before invoking this helper;
    those deliberately narrow operation bounds remain authoritative. All
    default and environment-selected nested budgets inherit the proof command's
    calibrated envelope as a floor, while still remaining bounded by that outer
    command owner.
    """

    owner_timeout = proof_command_timeout_seconds(environ)
    if owner_timeout is None:
        return selected_timeout
    if selected_timeout is None:
        return owner_timeout
    return max(selected_timeout, owner_timeout)


@dataclass(frozen=True, slots=True)
class CargoBuildResourcePolicy:
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


@dataclass(frozen=True, slots=True)
class CargoExecutionBudgetPolicy:
    timeout_seconds_by_class: Mapping[str, int]
    observed_cold_timeout_seconds: float
    minimum_cold_headroom_multiplier: float
    measurement_run_id: int
    measurement_job_id: int
    measurement_commit: str
    measurement_command: str

    def timeout_seconds(self, budget_class: str) -> int:
        try:
            return self.timeout_seconds_by_class[budget_class]
        except KeyError as exc:
            choices = ", ".join(sorted(self.timeout_seconds_by_class))
            raise ValueError(
                f"unknown Cargo execution budget {budget_class!r}; expected {choices}"
            ) from exc


@dataclass(frozen=True, slots=True)
class CargoEnvironmentPolicy:
    wrapper_environment_names: tuple[str, ...]
    incident_run_id: int
    incident_job_id: int
    incident_commit: str
    incident_command: str


@dataclass(frozen=True, slots=True)
class CiCargoPolicy:
    build_resources: CargoBuildResourcePolicy
    environment: CargoEnvironmentPolicy
    execution_budgets: CargoExecutionBudgetPolicy


def _positive_int(value: object, *, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise ValueError(f"CI Cargo policy {label} must be a positive integer")
    return value


def _positive_float(value: object, *, label: str) -> float:
    if (
        not isinstance(value, (int, float))
        or isinstance(value, bool)
        or not math.isfinite(float(value))
        or float(value) <= 0.0
    ):
        raise ValueError(f"CI Cargo policy {label} must be positive and finite")
    return float(value)


def _measurement_commit(value: object, *, label: str) -> str:
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{40}", value) is None:
        raise ValueError(f"CI Cargo policy {label} must be a lowercase 40-hex SHA")
    return value


def _measurement_command(value: object, *, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"CI Cargo policy {label} must be a non-empty string")
    return value


def load_ci_cargo_policy(
    path: Path = DEFAULT_CI_CARGO_POLICY,
) -> CiCargoPolicy:
    try:
        payload = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise ValueError(f"cannot read CI Cargo policy {path}: {exc}") from exc
    if payload.get("schema") != CI_CARGO_POLICY_SCHEMA:
        raise ValueError(f"CI Cargo policy schema must be {CI_CARGO_POLICY_SCHEMA!r}")
    if set(payload) != {
        "schema",
        "cargo_build",
        "cargo_environment",
        "cargo_execution",
    }:
        raise ValueError(
            "CI Cargo policy top-level fields must be exactly "
            "schema, cargo_build, cargo_environment, cargo_execution"
        )

    resources = payload.get("cargo_build")
    resource_fields = {
        "max_jobs",
        "measured_peak_rss_bytes",
        "headroom_ratio",
        "measurement_run_id",
        "measurement_commit",
        "measurement_command",
    }
    if not isinstance(resources, dict) or set(resources) != resource_fields:
        raise ValueError(
            "CI Cargo policy cargo_build fields must be exactly "
            + ", ".join(sorted(resource_fields))
        )
    headroom_ratio = _positive_float(
        resources["headroom_ratio"], label="cargo_build.headroom_ratio"
    )
    if not 0.10 <= headroom_ratio <= 2.0:
        raise ValueError(
            "CI Cargo policy cargo_build.headroom_ratio must be within [0.10, 2.0]"
        )
    build_resources = CargoBuildResourcePolicy(
        max_jobs=_positive_int(resources["max_jobs"], label="cargo_build.max_jobs"),
        measured_peak_rss_bytes=_positive_int(
            resources["measured_peak_rss_bytes"],
            label="cargo_build.measured_peak_rss_bytes",
        ),
        headroom_ratio=headroom_ratio,
        measurement_run_id=_positive_int(
            resources["measurement_run_id"],
            label="cargo_build.measurement_run_id",
        ),
        measurement_commit=_measurement_commit(
            resources["measurement_commit"],
            label="cargo_build.measurement_commit",
        ),
        measurement_command=_measurement_command(
            resources["measurement_command"],
            label="cargo_build.measurement_command",
        ),
    )

    environment = payload.get("cargo_environment")
    environment_fields = {
        "incident_run_id",
        "incident_job_id",
        "incident_commit",
        "incident_command",
    }
    if not isinstance(environment, dict) or set(environment) != environment_fields:
        raise ValueError(
            "CI Cargo policy cargo_environment fields must be exactly "
            + ", ".join(sorted(environment_fields))
        )
    environment_policy = CargoEnvironmentPolicy(
        wrapper_environment_names=CARGO_WRAPPER_ENV_NAMES,
        incident_run_id=_positive_int(
            environment["incident_run_id"],
            label="cargo_environment.incident_run_id",
        ),
        incident_job_id=_positive_int(
            environment["incident_job_id"],
            label="cargo_environment.incident_job_id",
        ),
        incident_commit=_measurement_commit(
            environment["incident_commit"],
            label="cargo_environment.incident_commit",
        ),
        incident_command=_measurement_command(
            environment["incident_command"],
            label="cargo_environment.incident_command",
        ),
    )

    execution = payload.get("cargo_execution")
    execution_fields = {
        "cross_check_timeout_seconds",
        "warm_timeout_seconds",
        "integration_timeout_seconds",
        "cold_timeout_seconds",
        "suite_timeout_seconds",
        "observed_cold_timeout_seconds",
        "minimum_cold_headroom_multiplier",
        "measurement_run_id",
        "measurement_job_id",
        "measurement_commit",
        "measurement_command",
    }
    if not isinstance(execution, dict) or set(execution) != execution_fields:
        raise ValueError(
            "CI Cargo policy cargo_execution fields must be exactly "
            + ", ".join(sorted(execution_fields))
        )
    timeout_seconds_by_class = {
        "cross-check": _positive_int(
            execution["cross_check_timeout_seconds"],
            label="cargo_execution.cross_check_timeout_seconds",
        ),
        "warm": _positive_int(
            execution["warm_timeout_seconds"],
            label="cargo_execution.warm_timeout_seconds",
        ),
        "integration": _positive_int(
            execution["integration_timeout_seconds"],
            label="cargo_execution.integration_timeout_seconds",
        ),
        "cold": _positive_int(
            execution["cold_timeout_seconds"],
            label="cargo_execution.cold_timeout_seconds",
        ),
        "suite": _positive_int(
            execution["suite_timeout_seconds"],
            label="cargo_execution.suite_timeout_seconds",
        ),
    }
    ordered = tuple(timeout_seconds_by_class.values())
    if ordered != tuple(sorted(ordered)):
        raise ValueError(
            "CI Cargo execution budgets must be monotonic: "
            "cross-check <= warm <= integration <= cold <= suite"
        )
    observed = _positive_float(
        execution["observed_cold_timeout_seconds"],
        label="cargo_execution.observed_cold_timeout_seconds",
    )
    multiplier = _positive_float(
        execution["minimum_cold_headroom_multiplier"],
        label="cargo_execution.minimum_cold_headroom_multiplier",
    )
    if timeout_seconds_by_class["cold"] < math.ceil(observed * multiplier):
        raise ValueError(
            "CI Cargo cold timeout is below the receipt-calibrated minimum headroom"
        )
    return CiCargoPolicy(
        build_resources=build_resources,
        environment=environment_policy,
        execution_budgets=CargoExecutionBudgetPolicy(
            timeout_seconds_by_class=timeout_seconds_by_class,
            observed_cold_timeout_seconds=observed,
            minimum_cold_headroom_multiplier=multiplier,
            measurement_run_id=_positive_int(
                execution["measurement_run_id"],
                label="cargo_execution.measurement_run_id",
            ),
            measurement_job_id=_positive_int(
                execution["measurement_job_id"],
                label="cargo_execution.measurement_job_id",
            ),
            measurement_commit=_measurement_commit(
                execution["measurement_commit"],
                label="cargo_execution.measurement_commit",
            ),
            measurement_command=_measurement_command(
                execution["measurement_command"],
                label="cargo_execution.measurement_command",
            ),
        ),
    )
