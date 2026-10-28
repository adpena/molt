"""Real publication observations of synthetic scoreboard-fixture artifacts.

Only test artifact bytes and selected profiles are observed. Compiler/runtime
identities remain unknown, and no compiled-with or used-byte admission is
claimed. Phase and release tests keep their real fail-closed toolchain checks.
"""

from __future__ import annotations

from contextlib import contextmanager
from collections.abc import Iterator, Sequence
from pathlib import Path
from typing import Any

from molt.cli.build_results import _observed_build_toolchain
from tools.perf_scoreboard_build_profiles import profile_selection_for_backend
from tools.bench_evidence import AdmittedBenchmarkWorkload


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
        "schema": "molt.benchmark-workload/2",
        "sealed_inputs": None,
        "entry": entry,
        "entry_source": Path(entry).name if Path(entry).is_absolute() else entry,
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


def measure_admitted_cell(measure, *, project_root: Path, **kwargs):
    """Exercise the real source admission around existing measurement fixtures."""
    from tools.bench_evidence import admit_benchmark_workload

    source = kwargs["script_path"]
    if not source.exists():
        source.parent.mkdir(parents=True, exist_ok=True)
        source.write_text("print(1)\n", encoding="utf-8")
    with admit_benchmark_workload(
        source,
        project_root=project_root,
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
                    project_root=profiling.REPO_ROOT,
                    run_args=profiling.bench.resolve_benchmark_run_args(str(path)),
                    molt_args=profiling.bench_suites.molt_args_for_benchmark(path),
                )
            )
            for path in kwargs["scripts"]
        }
        return profiling.run_hot_only_profiles(workloads=workloads, **kwargs)


@contextmanager
def sealed_workload(
    tmp_path: Path,
    *,
    molt_args: Sequence[str] = (),
    oracle_bytes: bytes = b"sealed:7\n",
) -> Iterator[AdmittedBenchmarkWorkload]:
    """Real sealed source/data/oracle custody for benchmark invocation tests."""
    from molt.cli.source_package_seal import (
        SourcePackageInput,
        stage_source_package_seal,
    )
    from tools.bench_evidence import admit_benchmark_workload

    original = tmp_path / "original"
    files = {
        "drivers/program.py": (
            "import sys\nfrom pathlib import Path\n"
            "from fixture_package import label\n"
            "print(label + Path(sys.argv[1]).read_text().strip() + ':' + sys.argv[2])\n"
        ),
        "packages/fixture_package/__init__.py": "label = 'seal'\n",
        "data/input.txt": "ed\n",
        "expected.txt": oracle_bytes,
        "pyproject.toml": '[project]\nname = "sealed-fixture"\nversion = "0.0.0"\n',
    }
    inputs = []
    for relative, content in files.items():
        source = original / relative
        source.parent.mkdir(parents=True, exist_ok=True)
        source.write_bytes(
            content.encode("utf-8") if isinstance(content, str) else content
        )
        role = (
            "oracle"
            if relative == "expected.txt"
            else "source"
            if relative.endswith(".py")
            else "data"
        )
        inputs.append(SourcePackageInput(source, relative, role))
    seal = stage_source_package_seal(tmp_path / "seals", inputs)
    with admit_benchmark_workload(
        original / "drivers/program.py",
        project_root=original,
        run_args=["data/input.txt", "7"],
        molt_args=molt_args,
        sealed_project=seal,
        python_roots=["packages"],
        oracle="expected.txt",
    ) as workload:
        yield workload
