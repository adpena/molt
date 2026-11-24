"""Shared benchmark metadata for Molt benchmark harnesses."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class BenchmarkReferenceContract:
    reference_runtime: str
    external_baselines: bool
    reason: str
    expected_stdout: str | None = None

    def molt_output_matches(self, stdout: str | None, stderr: str | None) -> bool:
        return self.expected_stdout is None or (
            stdout == self.expected_stdout and stderr == ""
        )


CPYTHON_REFERENCE = BenchmarkReferenceContract(
    reference_runtime="cpython",
    external_baselines=True,
    reason="cpython_reference",
)

_MOLT_INTRINSIC_OUTPUTS = {
    "tests/benchmarks/bench_channel_throughput.py": "1249975000\n",
    "tests/benchmarks/bench_ptr_registry.py": "100000\n",
}


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
    for key in _benchmark_keys(script):
        if key in _MOLT_INTRINSIC_OUTPUTS:
            return BenchmarkReferenceContract(
                reference_runtime="molt",
                external_baselines=False,
                reason="molt_runtime_intrinsics_without_external_reference",
                expected_stdout=_MOLT_INTRINSIC_OUTPUTS[key],
            )
    return CPYTHON_REFERENCE
