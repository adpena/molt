"""Shared benchmark JSON evidence policy."""

from __future__ import annotations

import math
from dataclasses import dataclass
from enum import Enum

from molt.compilation_target import TargetKind, admit_target, is_wasm_target
from molt.native_target import NativeTargetSpec
from typing import Any


class MetricLane(Enum):
    NATIVE = "native"
    WASM = "wasm"


@dataclass(frozen=True, slots=True)
class MetricPolicy:
    success_gates: tuple[str, ...]
    molt_lane: MetricLane | None = None


METRIC_POLICIES: dict[str, MetricPolicy] = {
    "codon_time_s": MetricPolicy(("codon_ok",)),
    "molt_build_s": MetricPolicy((), MetricLane.NATIVE),
    "molt_size_kb": MetricPolicy((), MetricLane.NATIVE),
    "molt_median_s": MetricPolicy((), MetricLane.NATIVE),
    "molt_codon_ratio": MetricPolicy(("molt_ok", "codon_ok"), MetricLane.NATIVE),
    "molt_cpython_ratio": MetricPolicy(("molt_ok",), MetricLane.NATIVE),
    "molt_nuitka_ratio": MetricPolicy(("molt_ok", "nuitka_ok"), MetricLane.NATIVE),
    "molt_pypy_ratio": MetricPolicy(("molt_ok", "pypy_ok"), MetricLane.NATIVE),
    "molt_pyodide_ratio": MetricPolicy(("molt_ok", "pyodide_ok"), MetricLane.NATIVE),
    "molt_speedup": MetricPolicy(("molt_ok",), MetricLane.NATIVE),
    "molt_time_s": MetricPolicy(("molt_ok",), MetricLane.NATIVE),
    "molt_wasm_build_s": MetricPolicy((), MetricLane.WASM),
    "molt_wasm_size_kb": MetricPolicy((), MetricLane.WASM),
    "molt_wasm_control_time_s": MetricPolicy(
        ("molt_wasm_control_ok",), MetricLane.WASM
    ),
    "molt_wasm_time_s": MetricPolicy(("molt_wasm_ok",), MetricLane.WASM),
    "molt_wasm_import_count": MetricPolicy((), MetricLane.WASM),
    "molt_wasm_function_import_count": MetricPolicy((), MetricLane.WASM),
    "molt_wasm_table_import_count": MetricPolicy((), MetricLane.WASM),
    "molt_wasm_function_imports_per_kb": MetricPolicy((), MetricLane.WASM),
    "nuitka_time_s": MetricPolicy(("nuitka_ok",)),
    "pypy_time_s": MetricPolicy(("pypy_ok",)),
    "pyodide_time_s": MetricPolicy(("pyodide_ok",)),
}


def _metric_lane_errors(
    cell: dict[str, Any], facts: dict[str, str], *, metric: str
) -> list[str]:
    policy = METRIC_POLICIES.get(metric)
    if policy is None or policy.molt_lane is None:
        return [f"unregistered Molt comparison metric: {metric}"]
    present = {
        declared.molt_lane
        for name, declared in METRIC_POLICIES.items()
        if declared.molt_lane is not None
        and isinstance(cell.get(name), (int, float))
        and not isinstance(cell[name], bool)
    }
    if len(present) > 1:
        return ["mixed native/WASM metric cell has one build observation"]
    try:
        target = admit_target(facts["target"])
    except (TypeError, ValueError) as exc:
        return [f"selected target is invalid for {metric}: {exc}"]
    native = target is TargetKind.NATIVE or isinstance(target, NativeTargetSpec)
    wasm = is_wasm_target(target)
    if (policy.molt_lane is MetricLane.NATIVE and not native) or (
        policy.molt_lane is MetricLane.WASM and not wasm
    ):
        return [
            f"{metric} requires {policy.molt_lane.value} target; observed {facts['target']!r}"
        ]
    return []


COMPARABLE_RUN_FIELDS: tuple[str, ...] = ("timing_mode", "warmup", "samples")


def valid_positive_number(value: Any) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    normalized = float(value)
    if not math.isfinite(normalized) or normalized <= 0:
        return None
    return normalized


def observed_build_profiles(observation: object) -> dict[str, str] | None:
    """Project the producer's selected coordinates; never resolve a request.

    This is publication observation, not loaded-byte admission. Historical,
    missing or malformed observations remain unbound, without an environment
    lookup or logical-to-physical profile conversion.
    """
    if (
        not isinstance(observation, dict)
        or observation.get("kind") != "molt-build-observation-v1"
    ):
        return None
    facts = observation.get("selected_profiles")
    if not isinstance(facts, dict) or set(facts) != {
        "backend",
        "target",
        "guest_profile",
        "runtime_profile",
        "compiler_profile",
    }:
        return None
    if any(
        not isinstance(value, str) or not value.strip() or value != value.strip()
        for value in facts.values()
    ):
        return None
    return dict(facts)


def format_observed_build_profiles(entry: dict[str, Any]) -> str:
    facts = observed_build_profiles(entry.get("build_observation"))
    if facts is None:
        return "unbound"
    return (
        f"{facts['backend']}/{facts['target']} "
        f"guest={facts['guest_profile']} runtime={facts['runtime_profile']} "
        f"compiler={facts['compiler_profile']}"
    )


def comparable_build_profile_errors(
    current: dict[str, Any], baseline: dict[str, Any], *, metric: str
) -> list[str]:
    policy = METRIC_POLICIES.get(metric)
    if policy is None:
        return (
            [f"unregistered Molt comparison metric: {metric}"]
            if metric.startswith("molt_")
            else []
        )
    if policy.molt_lane is None:
        return []
    errors: list[str] = []
    current_cells = current.get("benchmarks", {})
    baseline_cells = baseline.get("benchmarks", {})
    if not isinstance(current_cells, dict) or not isinstance(baseline_cells, dict):
        return ["benchmark profile observations require object-valued cells"]
    for name in sorted(set(current_cells) & set(baseline_cells)):
        current_cell, baseline_cell = current_cells[name], baseline_cells[name]
        if not isinstance(current_cell, dict) or not isinstance(baseline_cell, dict):
            errors.append(
                f"{name}: benchmark profile observations require object-valued cells"
            )
            continue
        observed: list[dict[str, str] | None] = []
        for role, cell in (("current", current_cell), ("baseline", baseline_cell)):
            facts = observed_build_profiles(cell.get("build_observation"))
            if facts is None:
                errors.append(f"{name}: {role} selected profiles unbound")
            elif (
                cell.get("requested_profile") is not None
                and cell["requested_profile"] != facts["guest_profile"]
            ):
                errors.append(
                    f"{name}: {role} requested guest profile differs from observation"
                )
            if facts is not None:
                errors.extend(
                    f"{name}: {role} {error}"
                    for error in _metric_lane_errors(cell, facts, metric=metric)
                )
            observed.append(facts)
        if all(facts is not None for facts in observed) and observed[0] != observed[1]:
            errors.append(
                f"{name}: selected build profiles differ: current={observed[0]!r}, baseline={observed[1]!r}"
            )
    return errors


def comparable_run_metadata_errors(
    current: dict[str, Any], baseline: dict[str, Any]
) -> list[str]:
    """Return fail-closed evidence-contract errors for benchmark comparisons."""
    errors: list[str] = []
    for field in COMPARABLE_RUN_FIELDS:
        cur_value = current.get(field)
        base_value = baseline.get(field)
        if cur_value is None:
            errors.append(f"current missing {field}")
            continue
        if base_value is None:
            errors.append(f"baseline missing {field}")
            continue
        if cur_value != base_value:
            errors.append(
                f"{field} differs: current={cur_value!r}, baseline={base_value!r}"
            )
    # Timing mismatch already refuses the comparison; preserve its precise
    # diagnostic before examining the independent profile binding.
    if not errors:
        errors.extend(
            comparable_build_profile_errors(current, baseline, metric="molt_time_s")
        )
    return errors


def metric_is_comparable(entry: dict[str, Any], metric: str) -> bool:
    policy = METRIC_POLICIES.get(metric)
    gates = policy.success_gates if policy is not None else ()
    return all(entry.get(gate) is True for gate in gates)


def comparator_time(entry: dict[str, Any], lane: str) -> float | None:
    if not entry.get(f"{lane}_ok"):
        return None
    return valid_positive_number(entry.get(f"{lane}_time_s"))


def native_molt_time(entry: dict[str, Any]) -> float | None:
    if not entry.get("molt_ok"):
        return None
    return valid_positive_number(entry.get("molt_time_s"))


def native_molt_speedup(entry: dict[str, Any]) -> float | None:
    if not entry.get("molt_ok"):
        return None
    return valid_positive_number(entry.get("molt_speedup"))


def wasm_molt_time(entry: dict[str, Any]) -> float | None:
    if not entry.get("molt_wasm_ok"):
        return None
    return valid_positive_number(entry.get("molt_wasm_time_s"))


def validated_runtime_samples(entry: dict[str, Any]) -> list[float] | None:
    if not entry.get("molt_ok"):
        return None

    raw_samples = entry.get("molt_samples_s")
    if raw_samples is None:
        stats = entry.get("super_stats", {}).get("molt")
        if stats is None:
            return None
        raw_samples = stats.get("samples_s")

    if raw_samples is None:
        return None
    if not isinstance(raw_samples, list) or not raw_samples:
        raise ValueError("invalid raw sample array for runtime metric")

    samples: list[float] = []
    for sample in raw_samples:
        value = valid_positive_number(sample)
        if value is None:
            raise ValueError(f"invalid raw sample for runtime metric: {sample!r}")
        samples.append(value)
    return samples
