"""Real publication observations of synthetic scoreboard-fixture artifacts.

Only test artifact bytes and selected profiles are observed. Compiler/runtime
identities remain unknown, and no compiled-with or used-byte admission is
claimed. Phase and release tests keep their real fail-closed toolchain checks.
"""

from __future__ import annotations

from contextlib import contextmanager
from collections.abc import Iterator, Sequence
from pathlib import Path
import sys
from typing import Any, Literal

from molt.cli.build_results import _observed_build_toolchain
from molt.python_interpreter import (
    BENCHMARK_PYTHON_ARGUMENTS,
    BenchmarkPythonHost,
    BenchmarkPythonMetadata,
    ManagedPythonOrigin,
    PythonInterpreter,
    benchmark_python_host,
    benchmark_python_metadata,
    parse_python_interpreter_request,
)
from tools.perf_scoreboard_build_profiles import profile_selection_for_backend
from tools.bench_evidence import AdmittedBenchmarkWorkload


def benchmark_oracle_interpreter(
    executable: str | Path | None = None,
    version: str = "3.12.16",
    *,
    sys_platform: str | None = None,
    machine: str | None = None,
    arch: str | None = None,
    pointer_bits: Literal[32, 64] | None = None,
) -> PythonInterpreter:
    """Model the resolver result at a fixture boundary without probing a Python."""
    local = benchmark_python_host()
    platform = sys_platform or local.sys_platform
    host = BenchmarkPythonHost.from_observation(
        platform, machine or local.machine, pointer_bits or local.pointer_bits
    )
    if arch is not None:
        host = BenchmarkPythonHost(
            host.sys_platform, host.machine, arch, host.pointer_bits
        )
    if executable is None:
        if platform == "win32" and sys.platform != "win32":
            executable = "C:/managed-python/python.exe"
        elif platform != "win32" and sys.platform == "win32":
            executable = "/managed-python/python"
        else:
            executable = Path(sys.executable).with_name("benchmark-oracle-python")
    path = Path(executable)
    uv = path.with_name("uv.exe" if platform == "win32" else "uv")
    request = parse_python_interpreter_request(version)
    return PythonInterpreter(
        (str(path), *BENCHMARK_PYTHON_ARGUMENTS),
        str(path),
        version,
        "CPython",
        ManagedPythonOrigin(request, path, uv, host),
    )


def benchmark_oracle_metadata(
    executable: str | Path | None = None,
    version: str = "3.12.16",
    *,
    sys_platform: str | None = None,
    machine: str | None = None,
    arch: str | None = None,
    pointer_bits: Literal[32, 64] | None = None,
) -> BenchmarkPythonMetadata:
    """Project synthetic managed oracle data for retained-record reader tests."""
    return benchmark_python_metadata(
        benchmark_oracle_interpreter(
            executable,
            version,
            sys_platform=sys_platform,
            machine=machine,
            arch=arch,
            pointer_bits=pointer_bits,
        )
    )


def synthetic_reference_evidence(benchmark: str) -> dict[str, object]:
    """Build fixture witnesses for the workload's declared reference contract."""
    from tools import perf_schema
    from tools.bench_metadata import benchmark_reference_contract

    contract = benchmark_reference_contract(benchmark)
    if not contract.external_baselines:
        expected = contract.expected_stdout
        assert expected is not None
        return {
            "cpython_ok": False,
            "warm_speedup": None,
            "verdict": perf_schema.VERDICT_CPY_INCOMPAT,
            "classification": None,
            "repeat_stability": None,
            "repeat_ci_lo": None,
            "repeat_ci_hi": None,
            "output_parity": perf_schema.molt_contract_parity_evidence(
                expected, [("molt:cold", expected, "", 0)]
            ),
        }
    return {
        "cpython_ok": True,
        "warm_speedup": 2.0,
        "verdict": perf_schema.VERDICT_GREEN,
        "classification": perf_schema.CLASS_GREEN,
        "repeat_stability": "STABLE_ABOVE",
        "repeat_ci_lo": 1.5,
        "repeat_ci_hi": 2.5,
        "output_parity": perf_schema.output_parity_evidence(
            reference_observations=[("cpython:cold", "result\n", "", 0)],
            molt_observations=[("molt:cold", "result\n", "", 0)],
        ),
    }


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
    driver_source: str | None = None,
) -> Iterator[AdmittedBenchmarkWorkload]:
    """Real sealed source/data/oracle custody for benchmark invocation tests."""
    from molt.cli.source_package_seal import (
        SourcePackageInput,
        stage_source_package_seal,
    )
    from tools.bench_evidence import admit_benchmark_workload

    original = tmp_path / "original"
    files = {
        "drivers/program.py": driver_source
        if driver_source is not None
        else (
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
