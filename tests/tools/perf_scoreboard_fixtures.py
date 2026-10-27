"""Real publication observations of synthetic scoreboard-fixture artifacts.

Only test artifact bytes and selected profiles are observed. Compiler/runtime
identities remain unknown, and no compiled-with or used-byte admission is
claimed. Phase and release tests keep their real fail-closed toolchain checks.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from molt.cli.build_results import _observed_build_toolchain
from tools.perf_scoreboard_build_profiles import profile_selection_for_backend


def write_synthetic_build_observation(
    artifact: Path, *, backend: str, profile: str
) -> dict[str, Any]:
    selection = profile_selection_for_backend(backend, profile)
    artifact.parent.mkdir(parents=True, exist_ok=True)
    artifact.write_bytes(b"synthetic scoreboard artifact; not executable\n")
    return _observed_build_toolchain(
        backend_bin=None,
        runtime_lib=None,
        output=artifact,
        selected_profiles=selection.as_record(),
    )


def synthetic_workload(
    entry: str = "tests/benchmarks/bench_fib.py",
) -> dict[str, object]:
    """Literal source identity for synthetic metric cells, never measured code."""
    return {
        "entry": entry,
        "scope": "local-python-source-closure",
        "sources": [
            {
                "path": Path(entry).name if Path(entry).is_absolute() else entry,
                "size": 9,
                "sha256": "cc42155088fca5730758db72b2a5bca33112a941dfaa2d43098ec422ce4ea213",
            }
        ],
        "topology_sha256": "",
        "run_args": [],
        "molt_args": [],
    }


def measure_admitted_cell(measure, **kwargs):
    """Exercise the real source admission around existing measurement fixtures."""
    from tools.bench_evidence import admit_benchmark_workload

    source = kwargs["script_path"]
    if not source.exists():
        source.parent.mkdir(parents=True, exist_ok=True)
        source.write_text("print(1)\n", encoding="utf-8")
    with admit_benchmark_workload(
        source,
        project_root=measure.REPO_ROOT,
        run_args=measure.bench.resolve_benchmark_run_args(str(source)),
        molt_args=measure.bench_suites.molt_args_for_benchmark(source),
    ) as workload:
        return measure.measure_cell(workload=workload, **kwargs)


def profile_admitted_sources(profiling, **kwargs):
    from contextlib import ExitStack
    from tools.bench_evidence import admit_benchmark_workload

    with ExitStack() as custody:
        workloads = {
            path: custody.enter_context(
                admit_benchmark_workload(
                    path,
                    project_root=path.parent,
                    run_args=profiling.bench.resolve_benchmark_run_args(str(path)),
                    molt_args=profiling.bench_suites.molt_args_for_benchmark(path),
                )
            )
            for path in kwargs["scripts"]
        }
        return profiling.run_hot_only_profiles(workloads=workloads, **kwargs)
