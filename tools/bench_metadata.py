"""Shared benchmark metadata for Molt benchmark harnesses."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class BenchmarkReferenceContract:
    reference_runtime: str
    external_baselines: bool
    reason: str


CPYTHON_REFERENCE = BenchmarkReferenceContract(
    reference_runtime="cpython",
    external_baselines=True,
    reason="cpython_reference",
)

MOLT_INTRINSIC_REFERENCE = BenchmarkReferenceContract(
    reference_runtime="molt",
    external_baselines=False,
    reason="molt_runtime_intrinsics_without_external_reference",
)

MOLT_ONLY_BENCHMARKS = frozenset(
    {
        "tests/benchmarks/bench_channel_throughput.py",
        "tests/benchmarks/bench_ptr_registry.py",
    }
)


def _benchmark_keys(script: str) -> set[str]:
    path = Path(script)
    keys = {path.as_posix()}
    repo_root = Path(__file__).resolve().parents[1]
    try:
        keys.add(path.resolve(strict=False).relative_to(repo_root).as_posix())
    except ValueError:
        pass
    return keys


def benchmark_reference_contract(script: str) -> BenchmarkReferenceContract:
    if _benchmark_keys(script) & MOLT_ONLY_BENCHMARKS:
        return MOLT_INTRINSIC_REFERENCE
    return CPYTHON_REFERENCE
