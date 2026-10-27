"""Shared benchmark JSON evidence policy."""

from __future__ import annotations

import math
from dataclasses import dataclass
from contextlib import ExitStack, contextmanager
from collections.abc import Callable, Iterator, Mapping, Sequence
from pathlib import Path, PurePosixPath, PureWindowsPath
from typing import Literal, TypedDict
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


def workload_map_problems(value: object) -> list[str]:
    """Validate every advertised workload, including added and removed entries."""
    if not isinstance(value, dict):
        return ["benchmark workloads require an object-valued collection"]
    errors: list[str] = []
    for name, cell in value.items():
        if not isinstance(name, str) or not isinstance(cell, dict):
            errors.append(
                "benchmark workloads require string keys and object-valued cells"
            )
            continue
        errors.extend(
            f"{name}: {problem}"
            for problem in workload_identity_problems(
                cell.get("workload"), expected_entry=name
            )
        )
    return errors


def comparable_metric_errors(
    current: dict[str, Any], baseline: dict[str, Any], *, metric: str
) -> list[str]:
    policy = METRIC_POLICIES.get(metric)
    if policy is None and metric.startswith("molt_"):
        return [f"unregistered Molt comparison metric: {metric}"]
    errors: list[str] = []
    current_cells = current.get("benchmarks", {})
    baseline_cells = baseline.get("benchmarks", {})
    for role, cells in (("current", current_cells), ("baseline", baseline_cells)):
        errors.extend(f"{role} {problem}" for problem in workload_map_problems(cells))
    if errors:
        return errors
    for name in sorted(set(current_cells) & set(baseline_cells)):
        current_cell, baseline_cell = current_cells[name], baseline_cells[name]
        if current_cell["workload"] != baseline_cell["workload"]:
            errors.append(f"{name}: workload identities differ")
        if policy is None or policy.molt_lane is None:
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
        errors.extend(comparable_metric_errors(current, baseline, metric="molt_time_s"))
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


class BenchmarkSourceRecord(TypedDict):
    path: str
    size: int
    sha256: str


class BenchmarkWorkloadRecord(TypedDict):
    entry: str
    scope: Literal["local-python-source-closure"]
    sources: list[BenchmarkSourceRecord]
    topology_sha256: str
    run_args: list[str]
    molt_args: list[str]


def workload_identity_problems(
    value: object, *, expected_entry: str | None = None
) -> list[str]:
    """One durable workload contract for every benchmark projection."""
    if not isinstance(value, Mapping) or set(value) != {
        "entry",
        "scope",
        "sources",
        "topology_sha256",
        "run_args",
        "molt_args",
    }:
        return ["workload identity is missing or has invalid fields"]
    if not isinstance(value["entry"], str) or not value["entry"]:
        return ["workload entry is invalid"]
    if expected_entry is not None and value["entry"] != expected_entry:
        return [
            f"benchmark name differs from admitted workload entry: {expected_entry}"
        ]
    if value["scope"] != "local-python-source-closure":
        return ["workload source scope is unsupported"]

    def digest(v: object, *, empty: bool = False) -> bool:
        return type(v) is str and (
            (empty and not v)
            or (len(v) == 64 and all(c in "0123456789abcdef" for c in v))
        )

    if not digest(value["topology_sha256"], empty=True):
        return ["workload topology digest is invalid"]
    rows = value["sources"]
    if not isinstance(rows, list) or not rows:
        return ["workload sources are missing"]
    names: list[str] = []
    for row in rows:
        if not isinstance(row, Mapping) or set(row) != {"path", "size", "sha256"}:
            return ["workload source record is invalid"]
        name = row["path"]
        if (
            not isinstance(name, str)
            or not name
            or name.startswith("/")
            or "\\" in name
            or any(p in {"", ".", ".."} for p in name.split("/"))
        ):
            return ["workload source path is not relative and canonical"]
        if type(row["size"]) is not int or row["size"] < 0 or not digest(row["sha256"]):
            return ["workload source content identity is invalid"]
        names.append(name)
    if names != sorted(set(names)):
        return ["workload source paths are duplicated or unordered"]
    entry = value["entry"]
    posix_entry, windows_entry = PurePosixPath(entry), PureWindowsPath(entry)
    relative_entry = (
        posix_entry.name
        if posix_entry.is_absolute()
        else windows_entry.name
        if windows_entry.is_absolute()
        else entry
    )
    if relative_entry not in names:
        return ["workload entry is absent from its captured source paths"]
    for field in ("run_args", "molt_args"):
        if not isinstance(value[field], list) or any(
            type(v) is not str for v in value[field]
        ):
            return [f"workload {field} must be an exact argument list"]
    return []


def workloads_comparable(left: object, right: object) -> bool:
    return (
        not workload_identity_problems(left)
        and not workload_identity_problems(right)
        and left == right
    )


@dataclass(frozen=True)
class AdmittedBenchmarkWorkload:
    """A live path generation, not an assertion of loaded-byte attestation."""

    script: Path
    _record: BenchmarkWorkloadRecord
    _verify: Callable[[], None]

    @property
    def record(self) -> BenchmarkWorkloadRecord:
        from copy import deepcopy

        return deepcopy(self._record)

    def verify(self) -> None:
        self._verify()


@contextmanager
def admit_benchmark_workload(
    script: Path,
    *,
    project_root: Path,
    run_args: Sequence[str],
    molt_args: Sequence[str],
) -> Iterator[AdmittedBenchmarkWorkload]:
    """Hold source generations through both runtimes without relocating code.

    Local Python imports are captured by the compiler's existing source-closure
    owner. Runtime-supplied stdlib and arbitrary data reads are not claimed as
    captured source. Verification runs outside measured child intervals.
    """
    from molt.cli.python_source_closure import (
        LocalPythonSourceClosure,
        local_python_import_closure,
        local_python_import_graph_transaction,
    )
    from molt.toolchain_identity import (
        open_stable_regular_file,
        stable_regular_file_handle_identity,
        verify_stable_regular_file_identity,
    )
    from tools.bench_suites import canonical_benchmark_key

    script = script.expanduser().absolute()
    project_root = project_root.resolve()
    root = project_root if script.is_relative_to(project_root) else script.parent
    search_roots = tuple(
        dict.fromkeys(p for p in (script.parent, root / "src", root) if p.is_dir())
    )

    def discover() -> LocalPythonSourceClosure:
        # Reuse content-keyed AST hints, never an earlier immutable query result.
        with local_python_import_graph_transaction(fresh=True):
            return local_python_import_closure(
                root, (script,), search_roots=search_roots
            )

    closure = discover()
    with ExitStack() as custody:
        identities = []
        for path in closure.paths:
            opened = custody.enter_context(
                open_stable_regular_file(path, label="benchmark source")
            )
            identity = stable_regular_file_handle_identity(
                opened, label="benchmark source"
            )
            if identity.sha256 != closure.source_sha256[path]:
                raise ValueError(f"benchmark source changed during admission: {path}")
            identities.append(identity)
        record: BenchmarkWorkloadRecord = {
            "entry": canonical_benchmark_key(script),
            "scope": "local-python-source-closure",
            "sources": sorted(
                [
                    {
                        "path": i.path.relative_to(root).as_posix(),
                        "size": i.size,
                        "sha256": i.sha256,
                    }
                    for i in identities
                ],
                key=lambda row: row["path"],
            ),
            "topology_sha256": closure.topology_digest,
            "run_args": list(run_args),
            "molt_args": list(molt_args),
        }
        problems = workload_identity_problems(record)
        if problems:
            raise ValueError("; ".join(problems))

        def verify() -> None:
            for identity in identities:
                verify_stable_regular_file_identity(
                    identity, label="benchmark source", hash_content=True
                )
            current = discover()
            if (
                current.paths != closure.paths
                or current.content_digest != closure.content_digest
                or current.topology_digest != closure.topology_digest
            ):
                raise ValueError(f"benchmark source closure changed: {script}")

        workload = AdmittedBenchmarkWorkload(script, record, verify)
        workload.verify()
        try:
            yield workload
        except BaseException as primary:
            try:
                workload.verify()
            except BaseException as invalidation:
                primary.add_note(f"workload invalidated during failure: {invalidation}")
            raise
        else:
            workload.verify()


def workload_collection_problems(cells: Sequence[Mapping[str, object]]) -> list[str]:
    identities: dict[str, object] = {}
    keys: set[tuple[object, ...]] = set()
    problems: list[str] = []
    for cell in cells:
        name = cell.get("benchmark")
        workload = cell.get("workload")
        problems.extend(
            workload_identity_problems(
                workload, expected_entry=name if isinstance(name, str) else None
            )
        )
        if not isinstance(name, str):
            problems.append("benchmark name is missing")
            continue
        if name in identities and not workloads_comparable(identities[name], workload):
            problems.append(f"benchmark has conflicting workload identities: {name}")
        identities[name] = workload
        key = tuple(cell.get(k) for k in ("benchmark", "target", "backend", "profile"))
        if any(type(value) is not str for value in key):
            problems.append(f"benchmark coordinate is invalid: {name}")
            continue
        if key in keys:
            problems.append(f"duplicate benchmark cell: {key}")
        keys.add(key)
    return problems
